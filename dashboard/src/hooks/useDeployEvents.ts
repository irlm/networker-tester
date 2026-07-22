import { useEffect, useRef, useState } from 'react';
import { useAuthStore } from '../stores/authStore';

// Reconnect backoff bounds (same as useApprovalSSE). Multi-minute Azure
// deploys watched through nginx are the worst case for proxy idle timeouts;
// without a reconnect loop the deploy log freezes mid-deploy. Replay after a
// reconnect is deduped by `lastSeqRef`.
const INITIAL_BACKOFF_MS = 1_000;
const MAX_BACKOFF_MS = 30_000;

export interface DeployLogLine {
  seq: number;
  line: string;
  stream: string;
}

export interface DeployCompleteEvent {
  seq: number;
  status: string;
  endpoint_ips: string[];
}

interface UseDeployEventsResult {
  /** Live log lines delivered via the SSE stream, in arrival order. */
  lines: string[];
  /** Set once the server emits a `DeployComplete` event for this deployment. */
  complete: DeployCompleteEvent | null;
  /** Stream connection state; useful for debugging + fallback UI. */
  connected: boolean;
}

/**
 * Subscribe to the per-deployment Server-Sent Events stream.
 *
 * Connects to `GET /api/projects/{projectId}/deployments/{deploymentId}/events`,
 * which carries every `DeployLog` and `DeployComplete` event for this specific
 * deployment (replays recent ring-buffer history first, then tails live).
 *
 * Why this hook exists alongside the dashboard-wide WebSocket: the WS fans out
 * every event in every project to every connected tab, so a busy benchmark in
 * project A can starve a client watching a deploy in project B. This stream
 * only carries the caller's deployment — it's the right scope for the
 * deploy-detail page, which otherwise has to filter a firehose.
 *
 * Uses `fetch` with streaming body + manual SSE parsing (not the native
 * `EventSource`) because the dashboard's JWT must be sent in the
 * `Authorization` header, which `EventSource` does not support.
 *
 * Returns stable references for `lines` and `complete` — safe to depend on
 * directly in effects without re-render loops.
 */
export function useDeployEvents(
  projectId: string | null,
  deploymentId: string | undefined,
): UseDeployEventsResult {
  const token = useAuthStore((s) => s.token);
  const [lines, setLines] = useState<string[]>([]);
  const [complete, setComplete] = useState<DeployCompleteEvent | null>(null);
  const [connected, setConnected] = useState(false);
  // Track the highest seq we've merged into `lines` so buffered replay +
  // live-tail can't introduce duplicates (server dedupes the per-stream
  // subscription window, but a client-side guard stays correct under any
  // future server changes).
  const lastSeqRef = useRef(0);
  // Reset per (project, deployment). If the user navigates from one deploy
  // to another the state belongs to the previous stream and must be dropped
  // before the new one mounts.
  const keyRef = useRef<string | null>(null);

  useEffect(() => {
    if (!projectId || !deploymentId || !token) return;

    const key = `${projectId}/${deploymentId}`;
    if (keyRef.current !== key) {
      keyRef.current = key;
      setLines([]);
      setComplete(null);
      lastSeqRef.current = 0;
    }

    const controller = new AbortController();
    let cancelled = false;
    let backoff = INITIAL_BACKOFF_MS;
    let retryTimer: ReturnType<typeof setTimeout> | null = null;

    const scheduleReconnect = () => {
      if (cancelled) return;
      const delay = backoff;
      backoff = Math.min(backoff * 2, MAX_BACKOFF_MS);
      retryTimer = setTimeout(() => void connect(), delay);
    };

    const connect = async () => {
      try {
        const res = await fetch(
          `/api/projects/${projectId}/deployments/${deploymentId}/events`,
          {
            headers: {
              Authorization: `Bearer ${token}`,
              Accept: 'text/event-stream',
            },
            signal: controller.signal,
          },
        );
        // Auth is dead — retrying with the same token is pointless. The next
        // REST call will handle the 401 redirect.
        if (res.status === 401 || res.status === 403) return;
        if (!res.ok || !res.body) {
          scheduleReconnect();
          return;
        }
        if (cancelled) return;
        setConnected(true);
        // Connected — reset backoff so the next drop retries quickly.
        backoff = INITIAL_BACKOFF_MS;

        const reader = res.body.getReader();
        const decoder = new TextDecoder();
        let buffer = '';

        for (;;) {
          const { done, value } = await reader.read();
          if (done) break;
          buffer += decoder.decode(value, { stream: true });
          // SSE frames are separated by a blank line (\n\n). Parse per-frame
          // so multi-line frames with multiple `data:` lines are handled
          // (they're rare in our stream but the protocol permits them).
          const frames = buffer.split('\n\n');
          buffer = frames.pop() || '';

          for (const frame of frames) {
            const dataLines = frame
              .split('\n')
              .filter((l) => l.startsWith('data: '))
              .map((l) => l.slice(6));
            if (dataLines.length === 0) continue;
            const payload = dataLines.join('\n');
            let data: Record<string, unknown>;
            try {
              data = JSON.parse(payload);
            } catch {
              continue;
            }
            const seq = typeof data.seq === 'number' ? data.seq : 0;
            if (seq <= lastSeqRef.current) continue;
            lastSeqRef.current = seq;

            if (data.type === 'deploy_log' && typeof data.line === 'string') {
              // Accumulate into a local copy so we only trigger one render
              // even if the TCP packet contained multiple frames.
              setLines((prev) => [...prev, data.line as string]);
            } else if (data.type === 'deploy_complete') {
              setComplete({
                seq,
                status: (data.status as string) ?? 'unknown',
                endpoint_ips: (data.endpoint_ips as string[]) ?? [],
              });
            }
          }
        }
        // Stream ended server-side (restart, proxy idle timeout) — reconnect.
        // The seq guard above dedupes the replayed ring-buffer history.
        if (cancelled) return;
        setConnected(false);
        scheduleReconnect();
      } catch (err) {
        // Guarded so an aborted StrictMode first-pass can't race the second
        // mount's setConnected(true).
        if (cancelled) return;
        setConnected(false);
        // AbortError on unmount / navigation is expected — anything else is
        // a real drop: log it and go through the reconnect path.
        if (err instanceof DOMException && err.name === 'AbortError') return;
        console.error('Deploy event stream failed', err);
        scheduleReconnect();
      }
    };

    void connect();

    return () => {
      cancelled = true;
      if (retryTimer) clearTimeout(retryTimer);
      controller.abort();
    };
  }, [projectId, deploymentId, token]);

  return { lines, complete, connected };
}
