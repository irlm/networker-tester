import { useEffect, useState } from 'react';

/**
 * A clock value that lives in state instead of being read during render.
 *
 * `Date.now()` called while rendering makes the component impure: the same
 * props and state produce a different tree on every render, which is what
 * `react-hooks/purity` objects to. Holding the timestamp in state keeps render
 * a pure function and still refreshes on a fixed cadence.
 *
 * @param intervalMs how often to advance the clock (default 30s — enough for
 *                   relative labels and expiry badges without waking the tab
 *                   more than necessary)
 */
export function useNow(intervalMs = 30_000): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), intervalMs);
    return () => clearInterval(id);
  }, [intervalMs]);
  return now;
}
