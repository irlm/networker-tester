import { useEffect } from 'react';

export function useHotkey(key: string, callback: () => void, enabled = true): void {
  useEffect(() => {
    if (!enabled) return;

    function handler(e: KeyboardEvent) {
      // Skip when typing in form elements
      const tag = (e.target as HTMLElement)?.tagName;
      if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT') return;
      if ((e.target as HTMLElement)?.isContentEditable) return;

      // Skip with modifier keys (allow Shift for ? which is Shift+/)
      if (e.ctrlKey || e.altKey || e.metaKey) return;

      if (e.key !== key) return;

      e.preventDefault();
      callback();
    }

    document.addEventListener('keydown', handler);
    return () => document.removeEventListener('keydown', handler);
  }, [key, callback, enabled]);
}
