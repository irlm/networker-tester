import { useEffect, useEffectEvent } from 'react';
import { setRequestSource } from '../lib/requestSource';

/**
 * NOTE: the `'poll'` request-source tag is only valid *synchronously* — it is
 * reset to `'user'` on the next microtask. Any api call `fn` issues after an
 * `await` (or in a `.then`) will be mis-tagged `'user'` in the perf log, so
 * poll callbacks must issue all api calls synchronously (e.g. a single
 * `Promise.all` of request() calls).
 *
 * @param resetKey Optional value that restarts the poll loop (and fires an
 *                 immediate tick) when it changes — used by retry buttons.
 */
export function usePolling(fn: () => void, intervalMs: number, enabled = true, resetKey: unknown = null) {
  const onTick = useEffectEvent(fn);

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
    run();
    const id = setInterval(run, intervalMs);
    return () => { cancelled = true; clearInterval(id); };
  }, [intervalMs, enabled, resetKey]);
}
