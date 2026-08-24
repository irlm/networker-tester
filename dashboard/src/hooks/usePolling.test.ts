// usePolling: interval loop + the visibilitychange fast-path (freshness
// audit) — returning to the tab must fire an immediate tick instead of
// waiting out the interval, and hiding the tab must NOT tick.
//
// Only setInterval/clearInterval are faked: the hook's request-source reset
// rides a real microtask (queueMicrotask), so faking those too would freeze
// the 'poll' tag forever and make the tagging assertion vacuous.

import { renderHook } from '@testing-library/react';
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { usePolling } from './usePolling';
import { getRequestSource } from '../lib/requestSource';

// jsdom pins document.visibilityState to 'visible'; redefine it per event so
// the hook's guard ("only tick when the state becomes visible") is exercised.
function setVisibility(state: DocumentVisibilityState) {
  Object.defineProperty(document, 'visibilityState', {
    configurable: true,
    get: () => state,
  });
  document.dispatchEvent(new Event('visibilitychange'));
}

describe('usePolling', () => {
  beforeEach(() => {
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval'] });
  });

  afterEach(() => {
    vi.useRealTimers();
    setVisibility('visible'); // restore jsdom's default for the next test
  });

  it('ticks immediately on mount, then every interval', () => {
    const fn = vi.fn();
    renderHook(() => usePolling(fn, 30_000));
    expect(fn).toHaveBeenCalledTimes(1); // the initial run covers mount
    vi.advanceTimersByTime(30_000);
    expect(fn).toHaveBeenCalledTimes(2);
  });

  it('fires an immediate tick when the tab becomes visible again', () => {
    const fn = vi.fn();
    renderHook(() => usePolling(fn, 30_000));
    expect(fn).toHaveBeenCalledTimes(1);
    setVisibility('hidden'); // leaving the tab must NOT tick
    expect(fn).toHaveBeenCalledTimes(1);
    setVisibility('visible'); // returning refreshes instantly
    expect(fn).toHaveBeenCalledTimes(2);
  });

  it('tags visibility-triggered ticks as poll requests (synchronously)', () => {
    const seen: string[] = [];
    renderHook(() => usePolling(() => seen.push(getRequestSource()), 30_000));
    setVisibility('hidden');
    setVisibility('visible');
    expect(seen).toEqual(['poll', 'poll']); // mount tick + visibility tick
  });

  it('removes the listener on unmount', () => {
    const fn = vi.fn();
    const { unmount } = renderHook(() => usePolling(fn, 30_000));
    unmount();
    setVisibility('hidden');
    setVisibility('visible');
    expect(fn).toHaveBeenCalledTimes(1); // only the mount tick
  });

  it('enabled=false: no ticks, not even on visibility changes', () => {
    const fn = vi.fn();
    renderHook(() => usePolling(fn, 30_000, false));
    vi.advanceTimersByTime(90_000);
    setVisibility('hidden');
    setVisibility('visible');
    expect(fn).not.toHaveBeenCalled();
  });

  // `immediate: false` — for pages that already load once themselves. Before
  // this option those pages issued EVERY request twice on mount; a prod
  // Infrastructure page load fired testers / deployments / vm-history /
  // cloud-accounts two times each (2026-08-24).
  describe('immediate: false', () => {
    it('skips the mount tick but still polls on the interval', () => {
      const fn = vi.fn();
      renderHook(() => usePolling(fn, 30_000, true, null, { immediate: false }));
      expect(fn).not.toHaveBeenCalled();
      vi.advanceTimersByTime(30_000);
      expect(fn).toHaveBeenCalledTimes(1);
    });

    it('STILL fires at once when resetKey changes — that is the Refresh button', () => {
      const fn = vi.fn();
      const { rerender } = renderHook(
        ({ key }) => usePolling(fn, 30_000, true, key, { immediate: false }),
        { initialProps: { key: 0 } },
      );
      expect(fn).not.toHaveBeenCalled();   // mount suppressed
      rerender({ key: 1 });
      expect(fn).toHaveBeenCalledTimes(1); // refresh is not suppressed
    });

    it('STILL fires at once when enabled flips on — that is un-pausing', () => {
      const fn = vi.fn();
      const { rerender } = renderHook(
        ({ on }) => usePolling(fn, 30_000, on, null, { immediate: false }),
        { initialProps: { on: true } },
      );
      expect(fn).not.toHaveBeenCalled();
      rerender({ on: false });
      rerender({ on: true });
      expect(fn).toHaveBeenCalledTimes(1);
    });

    it('leaves the default untouched — omitting the option still ticks on mount', () => {
      const fn = vi.fn();
      renderHook(() => usePolling(fn, 30_000, true, null, {}));
      expect(fn).toHaveBeenCalledTimes(1);
    });
  });
});
