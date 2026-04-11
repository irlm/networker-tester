import { useEffect, useState } from 'react';

/**
 * Subscribe to `phase_update` messages for a single entity on `/ws/testers`.
 *
 * Server protocol: `crates/networker-common/src/tester_messages.rs`.
 * Phase updates are broadcast to subscribers of the tester that owns the
 * benchmark, so callers must supply `testerId` (the tester running the
 * entity) plus the entity identity to filter on.
 */

export type Phase =
  | 'queued'
  | 'starting'
  | 'deploy'
  | 'running'
  | 'collect'
  | 'done';

export type Outcome = 'success' | 'partial_success' | 'failure' | 'cancelled';

export type PhaseState = {
  phase: Phase;
  outcome: Outcome | null;
  message: string | null;
  appliedStages: Phase[];
  seq: number;
};

type IncomingPhaseUpdate = {
  type: 'phase_update';
  project_id: string;
  entity_type: string;
  entity_id: string;
  seq: number;
  phase: Phase;
  outcome?: Outcome | null;
  message?: string | null;
  applied_stages?: Phase[];
};

function buildWsUrl(): string {
  const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
  const token = localStorage.getItem('token') || '';
  const params = new URLSearchParams();
  params.set('token', token);
  return `${protocol}//${window.location.host}/ws/testers?${params.toString()}`;
}

export function usePhaseSubscription(
  projectId: string | null | undefined,
  testerId: string | null | undefined,
  entityType: string,
  entityId: string | null | undefined,
): PhaseState | null {
  const [state, setState] = useState<PhaseState | null>(null);

  useEffect(() => {
    if (!projectId || !testerId || !entityId || !entityType) {
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
            tester_ids: [testerId],
          }),
        );
      });

      socket.addEventListener('message', (ev: MessageEvent) => {
        let msg: unknown;
        try {
          msg = JSON.parse(ev.data as string);
        } catch {
          return;
        }
        if (!msg || typeof msg !== 'object') return;
        const m = msg as Partial<IncomingPhaseUpdate> & { type?: string };
        if (m.type !== 'phase_update') return;
        if (m.entity_type !== entityType || m.entity_id !== entityId) return;
        if (typeof m.seq !== 'number' || !m.phase) return;

        const incoming: PhaseState = {
          phase: m.phase,
          outcome: (m.outcome ?? null) as Outcome | null,
          message: m.message ?? null,
          appliedStages: (m.applied_stages ?? []) as Phase[],
          seq: m.seq,
        };

        setState((prev) => {
          if (prev && incoming.seq <= prev.seq) return prev;
          return incoming;
        });
      });

      socket.addEventListener('close', () => {
        if (cancelled) return;
        // On reconnect, let snapshots drive re-sync; phase seq is monotonic
        // within a run so we don't reset it here.
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
        socket.onclose = null;
        try {
          socket.close();
        } catch {
          // ignore
        }
      }
    };
  }, [projectId, testerId, entityType, entityId]);

  return state;
}
