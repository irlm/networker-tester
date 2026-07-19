import { useEffect, useEffectEvent } from 'react';
import { setRequestSource } from '../lib/requestSource';

/**
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
