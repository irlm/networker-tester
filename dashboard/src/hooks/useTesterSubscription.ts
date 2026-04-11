import { useEffect, useState } from 'react';

/**
 * Subscription hooks for `/ws/testers`.
 *
 * Protocol: see `crates/networker-common/src/tester_messages.rs`.
 * Server sends snake_case tagged messages; client sends
 * `subscribe_tester_queue` on open.
 *
 * For MVP each hook owns its own WebSocket connection. A follow-up can
 * extract a shared manager to multiplex subscribers.
 */

export type QueueEntry = {
  config_id: string;
  name: string;
  position?: number;
  eta_seconds?: number;
};

export type TesterQueueState = {
  running: QueueEntry | null;
  queued: QueueEntry[];
  seq: number;
};

type IncomingMessage =
  | {
      type: 'tester_queue_snapshot';
      project_id: string;
      tester_id: string;
      seq: number;
      running?: QueueEntry | null;
      queued: QueueEntry[];
    }
  | {
      type: 'tester_queue_update';
      project_id: string;
      tester_id: string;
      seq: number;
      trigger: string;
      running?: QueueEntry | null;
      queued: QueueEntry[];
    }
  | { type: string; [k: string]: unknown };

function buildWsUrl(): string {
  const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
  const token = localStorage.getItem('token') || '';
  const params = new URLSearchParams();
  params.set('token', token);
  return `${protocol}//${window.location.host}/ws/testers?${params.toString()}`;
}

/**
 * Subscribe to queue snapshots + updates for a set of testers in a project.
 * Returns a map keyed by `tester_id`.
 *
 * - Drops `tester_queue_update` messages whose `seq` is <= the last-seen seq
 *   for that tester (within a single connection).
 * - On reconnect, the server re-sends `tester_queue_snapshot` which is
 *   treated as authoritative and resets `seq`.
 */
export function useTesterSubscription(
  projectId: string | null | undefined,
  testerIds: string[],
): Record<string, TesterQueueState> {
  const [state, setState] = useState<Record<string, TesterQueueState>>({});
  const testerIdsKey = testerIds.join(',');

  useEffect(() => {
    if (!projectId || testerIds.length === 0) {
      return;
    }

    let cancelled = false;
    let socket: WebSocket | null = null;
    let reconnectTimer: ReturnType<typeof setTimeout> | null = null;
    let backoff = 1000;

    const connect = () => {
      if (cancelled) return;
      try {
        socket = new WebSocket(buildWsUrl());
      } catch {
        scheduleReconnect();
        return;
      }

      socket.addEventListener('open', () => {
        if (cancelled || !socket) return;
        backoff = 1000;
        socket.send(
          JSON.stringify({
            type: 'subscribe_tester_queue',
            project_id: projectId,
            tester_ids: testerIds,
          }),
        );
      });

      socket.addEventListener('message', (ev: MessageEvent) => {
        let msg: IncomingMessage;
        try {
          msg = JSON.parse(ev.data as string) as IncomingMessage;
        } catch {
          return;
        }
        if (
          msg.type !== 'tester_queue_snapshot' &&
          msg.type !== 'tester_queue_update'
        ) {
          return;
        }
        const testerId = (msg as { tester_id: string }).tester_id;
        const seq = (msg as { seq: number }).seq;
        const running = ((msg as { running?: QueueEntry | null }).running ??
          null) as QueueEntry | null;
        const queued = ((msg as { queued?: QueueEntry[] }).queued ??
          []) as QueueEntry[];

        setState((prev) => {
          const existing = prev[testerId];
          if (
            msg.type === 'tester_queue_update' &&
            existing &&
            seq <= existing.seq
          ) {
            return prev;
          }
          return {
            ...prev,
            [testerId]: { running, queued, seq },
          };
        });
      });

      socket.addEventListener('close', () => {
        if (cancelled) return;
        scheduleReconnect();
      });

      socket.addEventListener('error', () => {
        socket?.close();
      });
    };

    const scheduleReconnect = () => {
      if (cancelled) return;
      const delay = Math.min(backoff, 15000);
      backoff = Math.min(backoff * 2, 30000);
      reconnectTimer = setTimeout(connect, delay);
    };

    connect();

    return () => {
      cancelled = true;
      if (reconnectTimer) clearTimeout(reconnectTimer);
      if (socket) {
        // Prevent the close handler from scheduling a reconnect after teardown.
        socket.onclose = null;
        try {
          socket.close();
        } catch {
          // ignore
        }
      }
    };
    // testerIdsKey collapses the array identity so we only reconnect on
    // meaningful changes.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [projectId, testerIdsKey]);

  return state;
}
