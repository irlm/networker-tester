import { useEffect, useEffectEvent, useRef } from 'react';
import { setRequestSource } from '../lib/requestSource';

/**
 * NOTE: the `'poll'` request-source tag is only valid *synchronously* — it is
 * reset to `'user'` on the next microtask. Any api call `fn` issues after an
 * `await` (or in a `.then`) will be mis-tagged `'user'` in the perf log, so
 * poll callbacks must issue all api calls synchronously (e.g. a single
 * `Promise.all` of request() calls).
 *
 * Also fires an immediate tick when the tab regains visibility
 * (visibilitychange → 'visible'), so a page the user returns to refreshes
 * instantly instead of waiting out the interval. The listener only fires on
 * actual state *changes*, so a page that mounts already-visible is covered by
 * the initial tick alone — no double-fire.
 *
 * @param resetKey Optional value that restarts the poll loop (and fires an
 *                 immediate tick) when it changes — used by retry buttons.
 * @param options  `immediate: false` suppresses the mount tick for a page that
 *                 already loads once itself.
 */
export function usePolling(
  fn: () => void,
  intervalMs: number,
  enabled = true,
  resetKey: unknown = null,
  options?: { immediate?: boolean },
) {
  const onTick = useEffectEvent(fn);
  // Default true: the mount tick IS the initial load for most callers.
  const immediate = options?.immediate ?? true;
  // `immediate: false` must suppress ONLY the mount tick. Later re-runs of this
  // effect are meaningful events — a resetKey bump is the Refresh button, and an
  // enabled flip is un-pausing — and both must still fire at once, or those
  // controls appear dead.
  const startedRef = useRef(false);

  useEffect(() => {
    if (!enabled) return;
    let cancelled = false;
    const run = () => {
      if (cancelled) return;
      setRequestSource('poll');
      onTick();
      // Reset to 'user' on next microtask so any subsequent
      // user-triggered calls are tagged correctly
      queueMicrotask(() => setRequestSource('user'));
    };
    // Pages that pair this with their own initial load (useAsyncEffect) were
    // issuing EVERY request twice on mount — confirmed in prod 2026-08-24,
    // where one Infrastructure page load fired testers / deployments /
    // vm-history / cloud-accounts two times each. Those pages opt out here
    // rather than dropping their own load, because their poll is deliberately
    // SILENT (it never re-raises the loading flag) — making it the first load
    // would cost the page its initial spinner.
    const isMountTick = !startedRef.current;
    startedRef.current = true;
    if (immediate || !isMountTick) {
      run();
    }
    const id = setInterval(run, intervalMs);
    // Refresh immediately on return to the tab — the interval keeps running
    // while hidden but its data is up to intervalMs stale the moment the user
    // looks. Guard on 'visible': the event also fires on hide.
    const onVisible = () => {
      if (document.visibilityState === 'visible') run();
    };
    document.addEventListener('visibilitychange', onVisible);
    return () => {
      cancelled = true;
      clearInterval(id);
      document.removeEventListener('visibilitychange', onVisible);
    };
  }, [intervalMs, enabled, resetKey, immediate]);
}
