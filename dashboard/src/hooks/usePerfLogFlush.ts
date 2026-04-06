import { useEffect, useRef } from 'react';
import { useApiLogStore } from '../stores/apiLogStore';
import type { PerfLogInput } from '../api/types';

const FLUSH_INTERVAL = 30_000; // 30 seconds
const SESSION_ID = `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 6)}`;

/**
 * Periodically flushes in-memory perf log entries to the backend.
 * Runs in the background — no UI impact.
 */
export function usePerfLogFlush() {
  const lastFlushedApi = useRef(0);
  const lastFlushedRender = useRef(0);

  useEffect(() => {
    const flush = async () => {
      const token = localStorage.getItem('token');
      if (!token) return;

      const store = useApiLogStore.getState();
      if (!store.enabled) return;

      // Collect new entries since last flush (entries are prepended, newest first)
      const newApiEntries = store.entries.filter(e => e.id > lastFlushedApi.current);
      const newRenderEntries = store.renderEntries.filter(e => e.id > lastFlushedRender.current);

      if (newApiEntries.length === 0 && newRenderEntries.length === 0) return;

      // Build payload
      const entries: PerfLogInput[] = [];

      for (const e of newApiEntries) {
        // Skip logging perf-log requests to avoid infinite loop
        if (e.path.includes('perf-log')) continue;
        entries.push({
          kind: 'api',
          timestamp: e.timestamp,
          method: e.method,
          path: e.path,
          status: e.status,
          total_ms: e.totalMs,
          server_ms: e.serverMs ?? undefined,
          network_ms: e.networkMs ?? undefined,
          source: e.source,
        });
      }

      for (const e of newRenderEntries) {
        entries.push({
          kind: 'render',
          timestamp: e.timestamp,
          component: e.component,
          trigger: e.trigger,
          render_ms: e.renderMs,
          item_count: e.itemCount ?? undefined,
        });
      }

      if (entries.length === 0) return;

      // Update cursors before sending (so we don't double-send on retry)
      if (newApiEntries.length > 0) {
        lastFlushedApi.current = Math.max(...newApiEntries.map(e => e.id));
      }
      if (newRenderEntries.length > 0) {
        lastFlushedRender.current = Math.max(...newRenderEntries.map(e => e.id));
      }

      // Send directly via fetch to avoid logging the log request itself
      try {
        await fetch('/api/perf-log', {
          method: 'POST',
          headers: {
            'Content-Type': 'application/json',
            Authorization: `Bearer ${token}`,
          },
          body: JSON.stringify({ session_id: SESSION_ID, entries }),
        });
      } catch {
        // Silent fail — perf logging is best-effort
      }
    };

    // Flush on interval
    const id = setInterval(flush, FLUSH_INTERVAL);
    // Also flush on page unload
    const handleUnload = () => flush();
    window.addEventListener('beforeunload', handleUnload);

    return () => {
      clearInterval(id);
      window.removeEventListener('beforeunload', handleUnload);
    };
  }, []);
}
