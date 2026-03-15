import { useEffect, useRef, useCallback, useState } from 'react';
import { useLiveStore } from '../stores/liveStore';

export type ConnectionStatus = 'connecting' | 'connected' | 'disconnected';

export function useWebSocket() {
  const wsRef = useRef<WebSocket | null>(null);
  const mountedRef = useRef(true);
  const reconnectTimeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const backoffRef = useRef(3000);
  const [status, setStatus] = useState<ConnectionStatus>('disconnected');
  const addEvent = useLiveStore((s) => s.addEvent);
  const addEventRef = useRef(addEvent);
  addEventRef.current = addEvent;

  const connect = useCallback(() => {
    if (!mountedRef.current) return;

    setStatus('connecting');
    const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
    const token = localStorage.getItem('token') || '';
    // The browser WebSocket API does not support custom headers, so the JWT
    // must be sent as a query parameter. Server-side mitigations required:
    //   1. Short-lived tokens (rotate on each WS connect)
    //   2. Strip the token from access logs
    //   3. Validate token server-side on upgrade and reject expired tokens
    const ws = new WebSocket(
      `${protocol}//${window.location.host}/ws/dashboard?token=${encodeURIComponent(token)}`
    );

    ws.onopen = () => {
      if (!mountedRef.current) {
        ws.close();
        return;
      }
      setStatus('connected');
      backoffRef.current = 3000; // reset on successful connect
    };

    ws.onmessage = (event) => {
      try {
        const data = JSON.parse(event.data);
        if (typeof data === 'object' && data !== null && typeof data.type === 'string') {
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
      reconnectTimeoutRef.current = setTimeout(connect, delay);
    };

    ws.onerror = () => {
      ws.close();
    };

    wsRef.current = ws;
  }, []);

  useEffect(() => {
    mountedRef.current = true;
    connect();
    return () => {
      mountedRef.current = false;
      if (reconnectTimeoutRef.current) {
        clearTimeout(reconnectTimeoutRef.current);
        reconnectTimeoutRef.current = null;
      }
      wsRef.current?.close();
    };
  }, [connect]);

  return status;
}
