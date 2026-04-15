import { useEffect, useRef, useState } from 'react';
import { useLiveStore } from '../stores/liveStore';
import { useProjectStore } from '../stores/projectStore';

export type ConnectionStatus = 'connecting' | 'connected' | 'disconnected';

export function useWebSocket() {
  const wsRef = useRef<WebSocket | null>(null);
  const mountedRef = useRef(true);
  const reconnectTimeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const backoffRef = useRef(3000);
  // Last event seq we've seen from the server. On reconnect we pass this as
  // `?since=<seq>` and the server replays any events we missed during the
  // drop. Reset only when the user switches projects (new auth context, new
  // fan-out scope — previous seqs are no longer meaningful).
  const lastSeqRef = useRef(0);
  // Whether we've ever completed a connection on this mount. Suppresses the
  // `since=` param on the very first connect so a fresh page load doesn't
  // receive the dashboard's full ring-buffer history — the pages that care
  // fetch their own initial state via REST, the live feed starts empty.
  const hasConnectedRef = useRef(false);
  const [status, setStatus] = useState<ConnectionStatus>('disconnected');
  const addEvent = useLiveStore((s) => s.addEvent);
  const addEventRef = useRef(addEvent);
  const activeProjectId = useProjectStore((s) => s.activeProjectId);
  const activeProjectIdRef = useRef(activeProjectId);
  const connectRef = useRef<() => void>(() => {});

  // Sync refs in effects instead of during render
  useEffect(() => { addEventRef.current = addEvent; }, [addEvent]);
  useEffect(() => { activeProjectIdRef.current = activeProjectId; }, [activeProjectId]);

  useEffect(() => {
    connectRef.current = () => {
      if (!mountedRef.current) return;

      setStatus('connecting');
      const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
      const token = localStorage.getItem('token') || '';
      const params = new URLSearchParams();
      params.set('token', token);
      const projectId = activeProjectIdRef.current;
      if (projectId) params.set('project_id', projectId);
      // Only request replay on *re*-connects. First connect on a fresh mount
      // starts the live feed empty (initial state comes from REST).
      if (hasConnectedRef.current && lastSeqRef.current > 0) {
        params.set('since', String(lastSeqRef.current));
      }
      const ws = new WebSocket(
        `${protocol}//${window.location.host}/ws/dashboard?${params.toString()}`
      );

      ws.onopen = () => {
        if (!mountedRef.current) {
          ws.close();
          return;
        }
        setStatus('connected');
        backoffRef.current = 3000;
        hasConnectedRef.current = true;
      };

      ws.onmessage = (event) => {
        try {
          const data = JSON.parse(event.data);
          if (typeof data === 'object' && data !== null && typeof data.type === 'string') {
            // Track the latest seq so a reconnect can request replay. Skip
            // anything with seq <= our watermark (defensive — the server
            // dedupes during replay, but a client-side guard keeps us honest
            // if event delivery order ever changes).
            if (typeof data.seq === 'number') {
              if (data.seq <= lastSeqRef.current) return;
              lastSeqRef.current = data.seq;
            }
            addEventRef.current(data);
          }
        } catch {
          // ignore malformed messages
        }
      };

      ws.onclose = () => {
        setStatus('disconnected');
        if (!mountedRef.current) return;
        const delay = backoffRef.current;
        backoffRef.current = Math.min(backoffRef.current * 2, 60000);
        reconnectTimeoutRef.current = setTimeout(() => connectRef.current(), delay);
      };

      ws.onerror = () => {
        ws.close();
      };

      wsRef.current = ws;
    };
  });

  useEffect(() => {
    mountedRef.current = true;
    // Close any stale connection from a previous mount (StrictMode double-mount)
    if (wsRef.current) {
      wsRef.current.onclose = null;
      wsRef.current.close();
      wsRef.current = null;
    }
    if (reconnectTimeoutRef.current) {
      clearTimeout(reconnectTimeoutRef.current);
      reconnectTimeoutRef.current = null;
    }
    connectRef.current();
    return () => {
      mountedRef.current = false;
      if (reconnectTimeoutRef.current) {
        clearTimeout(reconnectTimeoutRef.current);
        reconnectTimeoutRef.current = null;
      }
      if (wsRef.current) {
        wsRef.current.onclose = null;
        wsRef.current.close();
        wsRef.current = null;
      }
    };
  }, []);

  // Reconnect when project changes
  useEffect(() => {
    // Skip initial mount — the effect above handles that
    if (!wsRef.current) return;
    if (wsRef.current.readyState === WebSocket.OPEN || wsRef.current.readyState === WebSocket.CONNECTING) {
      wsRef.current.onclose = null;
      wsRef.current.close();
      wsRef.current = null;
    }
    if (reconnectTimeoutRef.current) {
      clearTimeout(reconnectTimeoutRef.current);
      reconnectTimeoutRef.current = null;
    }
    backoffRef.current = 3000;
    // Project switch = new fan-out scope. Previous seqs are no longer
    // meaningful; start fresh so we don't request replay of events that
    // belong to a different project's context.
    lastSeqRef.current = 0;
    hasConnectedRef.current = false;
    connectRef.current();
  }, [activeProjectId]);

  return status;
}
