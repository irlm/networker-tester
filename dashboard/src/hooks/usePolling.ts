import { useEffect, useEffectEvent } from 'react';
import { setRequestSource } from '../lib/requestSource';

export function usePolling(fn: () => void, intervalMs: number, enabled = true) {
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
  }, [intervalMs, enabled]);
}
