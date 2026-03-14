import { useEffect, useRef, useCallback } from 'react';
import { useLiveStore } from '../stores/liveStore';

export function useWebSocket() {
  const wsRef = useRef<WebSocket | null>(null);
  const addEvent = useLiveStore((s) => s.addEvent);

  const connect = useCallback(() => {
    const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
    const ws = new WebSocket(`${protocol}//${window.location.host}/ws/dashboard`);

    ws.onopen = () => {
      console.log('Dashboard WebSocket connected');
    };

    ws.onmessage = (event) => {
      try {
        const data = JSON.parse(event.data);
        addEvent(data);
      } catch {
        // ignore malformed messages
      }
    };

    ws.onclose = () => {
      console.log('Dashboard WebSocket disconnected, reconnecting in 3s...');
      setTimeout(connect, 3000);
    };

    ws.onerror = () => {
      ws.close();
    };

    wsRef.current = ws;
  }, [addEvent]);

  useEffect(() => {
    connect();
    return () => {
      wsRef.current?.close();
    };
  }, [connect]);
}
