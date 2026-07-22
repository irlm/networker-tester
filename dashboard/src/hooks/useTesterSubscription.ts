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

// Reconnect backoff cap (delay and growth share one constant — they were
// previously 15s/30s, leaving the larger cap unreachable dead logic).
const MAX_BACKOFF_MS = 30_000;
// WS close codes that mean "auth rejected" — retrying with the same token is
// pointless. 1008 = policy violation, 4401 = app-level 401 convention.
const AUTH_CLOSE_CODES = new Set([1008, 4401]);
// A pre-upgrade 401 (expired JWT) surfaces in the browser as a generic close
// (1006) with no `open` event, indistinguishable from a server restart. After
// this many consecutive never-opened attempts, assume the token is dead and
// stop hammering — the next REST call's 401 handles the re-login redirect.
const MAX_CONSECUTIVE_FAILURES = 5;

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
    // Consecutive attempts that closed without ever opening — the signature
    // of a pre-upgrade auth rejection (see MAX_CONSECUTIVE_FAILURES).
    let consecutiveFailures = 0;
    let openedThisAttempt = false;

    const connect = () => {
      if (cancelled) return;
      openedThisAttempt = false;
      try {
        socket = new WebSocket(buildWsUrl());
      } catch {
        scheduleReconnect();
        return;
      }

      socket.addEventListener('open', () => {
        if (cancelled || !socket) return;
        openedThisAttempt = true;
        consecutiveFailures = 0;
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

      socket.addEventListener('close', (ev: CloseEvent) => {
        if (cancelled) return;
        // Server rejected auth — a retry with the same token can only fail.
        if (AUTH_CLOSE_CODES.has(ev.code)) return;
        if (!openedThisAttempt) {
          consecutiveFailures += 1;
          // Likely a dead token being 401'd pre-upgrade — stop hammering.
          if (consecutiveFailures >= MAX_CONSECUTIVE_FAILURES) return;
        }
        scheduleReconnect();
      });

      socket.addEventListener('error', () => {
        socket?.close();
      });
    };

    const scheduleReconnect = () => {
      if (cancelled) return;
      const delay = Math.min(backoff, MAX_BACKOFF_MS);
      backoff = Math.min(backoff * 2, MAX_BACKOFF_MS);
      reconnectTimer = setTimeout(connect, delay);
    };

    connect();

    return () => {
      cancelled = true;
      if (reconnectTimer) clearTimeout(reconnectTimer);
      if (socket) {
        // The close handler was attached via addEventListener, so it still
        // fires on this close() — the `cancelled` flag above (checked first
        // in the handler) is what actually prevents a post-teardown
        // reconnect. Intentional: no removeEventListener needed.
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
