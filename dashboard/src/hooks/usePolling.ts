import { useEffect, useEffectEvent } from 'react';

export function usePolling(fn: () => void, intervalMs: number, enabled = true) {
  const onTick = useEffectEvent(fn);

  useEffect(() => {
    if (!enabled) return;
    let cancelled = false;
    const run = () => { if (!cancelled) onTick(); };
    run();
    const id = setInterval(run, intervalMs);
    return () => { cancelled = true; clearInterval(id); };
  }, [intervalMs, enabled]);
}
