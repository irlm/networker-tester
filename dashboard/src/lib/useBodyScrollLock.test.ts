import { describe, expect, test, beforeEach } from 'vitest';
import { renderHook } from '@testing-library/react';
import { useBodyScrollLock, __resetBodyScrollLock } from './useBodyScrollLock';

describe('useBodyScrollLock', () => {
  beforeEach(() => {
    __resetBodyScrollLock();
  });

  test('locks while active and restores on unmount', () => {
    const { unmount } = renderHook(() => useBodyScrollLock(true));
    expect(document.body.style.overflow).toBe('hidden');
    unmount();
    expect(document.body.style.overflow).toBe('');
  });

  test('does nothing while inactive', () => {
    renderHook(() => useBodyScrollLock(false));
    expect(document.body.style.overflow).toBe('');
  });

  // The bug this hook exists to prevent: two overlays open at once (a modal
  // over the mobile drawer). With a plain save/restore, whichever unmounts
  // first puts back the value IT captured and unlocks the page behind the
  // overlay that is still open.
  test('stays locked until the LAST holder releases', () => {
    const first = renderHook(() => useBodyScrollLock(true));
    const second = renderHook(() => useBodyScrollLock(true));
    expect(document.body.style.overflow).toBe('hidden');

    first.unmount();
    expect(document.body.style.overflow).toBe('hidden');

    second.unmount();
    expect(document.body.style.overflow).toBe('');
  });

  test("restores the page's own overflow, not a blank value", () => {
    document.body.style.overflow = 'clip';
    const { unmount } = renderHook(() => useBodyScrollLock(true));
    expect(document.body.style.overflow).toBe('hidden');
    unmount();
    expect(document.body.style.overflow).toBe('clip');
  });

  test('toggling active re-acquires cleanly', () => {
    const { rerender, unmount } = renderHook(({ open }) => useBodyScrollLock(open), {
      initialProps: { open: true },
    });
    expect(document.body.style.overflow).toBe('hidden');
    rerender({ open: false });
    expect(document.body.style.overflow).toBe('');
    rerender({ open: true });
    expect(document.body.style.overflow).toBe('hidden');
    unmount();
    expect(document.body.style.overflow).toBe('');
  });
});
