import { useEffect, useRef } from 'react';
import { useAuthStore } from '../stores/authStore';

// Reconnect backoff bounds. The stream is long-lived but proxies and server
// restarts drop it; without a reconnect loop, approval notifications silently
// stop until a full page reload.
const INITIAL_BACKOFF_MS = 1_000;
const MAX_BACKOFF_MS = 30_000;

export function useApprovalSSE(onApproval: (data: Record<string, unknown>) => void) {
  const token = useAuthStore(s => s.token);
  const callbackRef = useRef(onApproval);
  callbackRef.current = onApproval;

  useEffect(() => {
    if (!token) return;
    const controller = new AbortController();
    let cancelled = false;
    let backoff = INITIAL_BACKOFF_MS;
    let retryTimer: ReturnType<typeof setTimeout> | null = null;

    const connect = async () => {
      try {
        const res = await fetch('/api/events/approval', {
          headers: {
            Authorization: `Bearer ${token}`,
            Accept: 'text/event-stream',
          },
          signal: controller.signal,
        });
        // Auth is dead — retrying with the same token is pointless. The next
        // REST call will handle the 401 redirect.
        if (res.status === 401 || res.status === 403) return;
        if (!res.ok || !res.body) {
          scheduleReconnect();
          return;
        }

        // Connected — reset backoff so the next drop retries quickly.
        backoff = INITIAL_BACKOFF_MS;

        const reader = res.body.getReader();
        const decoder = new TextDecoder();
        let buffer = '';

        for (;;) {
          const { done, value } = await reader.read();
          if (done) break;
          buffer += decoder.decode(value, { stream: true });
          const lines = buffer.split('\n');
          buffer = lines.pop() || '';

          for (const line of lines) {
            if (line.startsWith('data: ')) {
              try {
                const data = JSON.parse(line.slice(6));
                callbackRef.current(data);
              } catch {
                // Ignore malformed JSON
              }
            }
          }
        }
        // Stream ended server-side (restart, proxy idle timeout) — reconnect.
        scheduleReconnect();
      } catch {
        // AbortError on unmount is expected; anything else (network drop)
        // goes through the same reconnect path.
        scheduleReconnect();
      }
    };

    const scheduleReconnect = () => {
      if (cancelled) return;
      const delay = backoff;
      backoff = Math.min(backoff * 2, MAX_BACKOFF_MS);
      retryTimer = setTimeout(() => void connect(), delay);
    };

    void connect();

    return () => {
      cancelled = true;
      if (retryTimer) clearTimeout(retryTimer);
      controller.abort();
    };
  }, [token]);
}
