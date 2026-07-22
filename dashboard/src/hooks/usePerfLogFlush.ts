import { useEffect, useRef } from 'react';
import { useApiLogStore } from '../stores/apiLogStore';
import type { PerfLogInput } from '../api/types';

const FLUSH_INTERVAL = 30_000; // 30 seconds
const SESSION_ID = `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 6)}`;
// Browsers cap `keepalive: true` request bodies at ~64 KB (rejected
// synchronously above that). Chunk with margin so the pagehide flush — the
// one carrying the page's whole session — can't exceed it.
const MAX_BODY_BYTES = 60_000;

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

      // Build payload, keeping each entry's origin so cursors can be advanced
      // per successfully-flushed chunk (and only then — a failed flush must
      // leave the cursor behind so the next interval retries the entries).
      const tagged: { entry: PerfLogInput; kind: 'api' | 'render'; id: number }[] = [];

      for (const e of newApiEntries) {
        // Skip logging perf-log requests to avoid infinite loop
        if (e.path.includes('perf-log')) continue;
        tagged.push({
          kind: 'api',
          id: e.id,
          entry: {
            kind: 'api',
            timestamp: e.timestamp,
            method: e.method,
            path: e.path,
            status: e.status,
            total_ms: e.totalMs,
            server_ms: e.serverMs ?? undefined,
            network_ms: e.networkMs ?? undefined,
            source: e.source,
          },
        });
      }

      for (const e of newRenderEntries) {
        tagged.push({
          kind: 'render',
          id: e.id,
          entry: {
            kind: 'render',
            timestamp: e.timestamp,
            component: e.component,
            trigger: e.trigger,
            render_ms: e.renderMs,
            item_count: e.itemCount ?? undefined,
          },
        });
      }

      // Send directly via fetch to avoid logging the log request itself.
      // keepalive lets the request survive page unload/navigation — without
      // it the final flush (often the one carrying the page's whole session)
      // is aborted by the browser and silently lost. Chunked to stay under
      // the keepalive body cap (see MAX_BODY_BYTES).
      const envelopeBytes = JSON.stringify({ session_id: SESSION_ID, entries: [] }).length;
      let i = 0;
      while (i < tagged.length) {
        const chunk: typeof tagged = [];
        let size = envelopeBytes;
        while (i < tagged.length) {
          // +1 for the separating comma; string length ≈ byte length for our
          // ASCII-dominated payloads, and the 4 KB margin absorbs the rest.
          const entryBytes = JSON.stringify(tagged[i].entry).length + 1;
          if (chunk.length > 0 && size + entryBytes > MAX_BODY_BYTES) break;
          size += entryBytes;
          chunk.push(tagged[i]);
          i += 1;
        }
        try {
          const res = await fetch('/api/perf-log', {
            method: 'POST',
            headers: {
              'Content-Type': 'application/json',
              Authorization: `Bearer ${token}`,
            },
            body: JSON.stringify({ session_id: SESSION_ID, entries: chunk.map(c => c.entry) }),
            keepalive: true,
          });
          // Leave cursors behind on failure so the next interval retries
          // (server-side dedup by session_id + timestamp covers double-sends).
          if (!res.ok) return;
        } catch {
          // Silent fail — perf logging is best-effort; retried next interval.
          return;
        }
        // Advance cursors only past what the server accepted.
        for (const c of chunk) {
          if (c.kind === 'api') {
            lastFlushedApi.current = Math.max(lastFlushedApi.current, c.id);
          } else {
            lastFlushedRender.current = Math.max(lastFlushedRender.current, c.id);
          }
        }
      }

      // Everything flushed — also skip past any perf-log noise entries that
      // were filtered out above so they aren't re-scanned every interval.
      if (newApiEntries.length > 0) {
        lastFlushedApi.current = Math.max(lastFlushedApi.current, ...newApiEntries.map(e => e.id));
      }
      if (newRenderEntries.length > 0) {
        lastFlushedRender.current = Math.max(lastFlushedRender.current, ...newRenderEntries.map(e => e.id));
      }
    };

    // Flush on interval
    const id = setInterval(flush, FLUSH_INTERVAL);
    // Flush when the page is hidden or being unloaded. `visibilitychange` →
    // hidden is the reliable end-of-session signal (fires on tab switch,
    // navigation, and mobile background); `pagehide` covers bfcache
    // navigations where beforeunload never fires.
    const handleVisibility = () => {
      if (document.visibilityState === 'hidden') void flush();
    };
    const handlePageHide = () => void flush();
    document.addEventListener('visibilitychange', handleVisibility);
    window.addEventListener('pagehide', handlePageHide);

    return () => {
      clearInterval(id);
      document.removeEventListener('visibilitychange', handleVisibility);
      window.removeEventListener('pagehide', handlePageHide);
    };
  }, []);
}
