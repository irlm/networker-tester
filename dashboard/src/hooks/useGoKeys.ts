import { useEffect, useRef } from 'react';
import { useNavigate } from 'react-router';
import { useAuthStore } from '../stores/authStore';
import { useProjectStore } from '../stores/projectStore';
import { useDocsStore } from '../stores/docsStore';
import { NAV_ENTRIES, navEntryVisible, resolveNavPath } from '../components/layout/nav-config';

/**
 * `g`+key jump navigation (the CLI keyboard layer): press `g`, then a page
 * key within 800 ms — `g r` → Runs, `g d` → Dashboard. Key assignments live
 * on the nav entries in nav-config.ts, so the sidebar, the palette, and
 * this hook can never disagree about where a page lives.
 *
 * Inert while typing in a form element and while the palette/help overlays
 * own the keyboard (the palette has its own vim keys, including `gg`).
 */
export function useGoKeys(): void {
  const navigate = useNavigate();
  const pendingUntil = useRef(0);

  useEffect(() => {
    function handler(e: KeyboardEvent) {
      const tag = (e.target as HTMLElement)?.tagName;
      if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT') return;
      if ((e.target as HTMLElement)?.isContentEditable) return;
      if (e.ctrlKey || e.altKey || e.metaKey || e.shiftKey) return;

      const docs = useDocsStore.getState();
      if (docs.paletteOpen || docs.helpOpen) return;

      const now = Date.now();

      if (e.key === 'g') {
        pendingUntil.current = now + 800;
        return;
      }

      if (now > pendingUntil.current) return;
      pendingUntil.current = 0;

      const auth = useAuthStore.getState();
      const pid = useProjectStore.getState().activeProjectId ?? null;
      const ctx = {
        pid,
        isAdmin: auth.role === 'admin' || auth.isPlatformAdmin,
        isPlatformAdmin: auth.isPlatformAdmin,
      };

      const entry = NAV_ENTRIES.find(
        n => n.gKey === e.key && navEntryVisible(n, ctx),
      );
      if (!entry) return;

      const path = resolveNavPath(entry.path, pid);
      if (!path) return;

      e.preventDefault();
      navigate(path);
    }

    document.addEventListener('keydown', handler);
    return () => document.removeEventListener('keydown', handler);
  }, [navigate]);
}
