import { useEffect, type DependencyList } from 'react';

/**
 * Run an async loader from an effect without a synchronous setState.
 *
 * The codebase's data-loading shape is `useEffect(() => { void refresh() })`
 * where `refresh` sets `loading`/`error` before awaiting. React's
 * `react-hooks/set-state-in-effect` rule flags that: a state update during the
 * effect's synchronous body forces a cascading render before paint.
 *
 * This defers the loader past the effect's synchronous phase, so its updates
 * land as an ordinary async update — and adds the piece almost every call site
 * was missing: a cancellation flag, so a loader that resolves after unmount (or
 * after the deps changed) no longer sets state on a dead component.
 *
 * @param load  async loader; receives a `cancelled()` probe for long loads
 * @param deps  same semantics as useEffect's dependency list
 */
export function useAsyncEffect(
  load: (cancelled: () => boolean) => Promise<unknown> | void,
  deps: DependencyList,
): void {
  useEffect(() => {
    let cancelled = false;
    const isCancelled = () => cancelled;
    // Promise.resolve() moves the call out of the effect's synchronous body.
    void Promise.resolve().then(() => {
      if (cancelled) return;
      return load(isCancelled);
    });
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, deps);
}
