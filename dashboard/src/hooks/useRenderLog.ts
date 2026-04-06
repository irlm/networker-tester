import { useRef, useLayoutEffect, useCallback } from 'react';
import { useApiLogStore } from '../stores/apiLogStore';

/**
 * Measures time from a state change to React paint completion.
 *
 * Call `markRender(trigger, itemCount)` BEFORE setState.
 * The hook measures time to the next paint via rAF.
 */
export function useRenderLog(component: string) {
  const pendingRef = useRef<{
    trigger: string;
    start: number;
    itemCount: number | null;
  } | null>(null);

  // useLayoutEffect fires synchronously after DOM mutations but before paint.
  // Only schedule measurement when there's a pending mark.
  useLayoutEffect(() => {
    const pending = pendingRef.current;
    if (!pending) return;

    // Single rAF — fires right after the browser paints
    const id = requestAnimationFrame(() => {
      const renderMs = performance.now() - pending.start;
      const store = useApiLogStore.getState();
      if (store.enabled) {
        store.addRender({
          timestamp: Date.now(),
          component,
          trigger: pending.trigger,
          renderMs,
          itemCount: pending.itemCount,
        });
      }
      pendingRef.current = null;
    });

    return () => cancelAnimationFrame(id);
  });

  return useCallback(function markRender(trigger: string, itemCount?: number) {
    pendingRef.current = {
      trigger,
      start: performance.now(),
      itemCount: itemCount ?? null,
    };
  }, []);
}
