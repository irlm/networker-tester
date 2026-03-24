import { useEffect, useRef } from 'react';
import { useAuthStore } from '../stores/authStore';

export function useApprovalSSE(onApproval: (data: Record<string, unknown>) => void) {
  const token = useAuthStore(s => s.token);
  const callbackRef = useRef(onApproval);
  callbackRef.current = onApproval;

  useEffect(() => {
    if (!token) return;
    const controller = new AbortController();

    (async () => {
      try {
        const res = await fetch('/api/events/approval', {
          headers: {
            Authorization: `Bearer ${token}`,
            Accept: 'text/event-stream',
          },
          signal: controller.signal,
        });
        if (!res.ok || !res.body) return;

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
      } catch {
        // Connection closed or aborted
      }
    })();

    return () => controller.abort();
  }, [token]);
}
