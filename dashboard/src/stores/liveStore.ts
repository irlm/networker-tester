import { create } from 'zustand';

export interface DashboardEvent {
  type: string;
  job_id?: string;
  status?: string;
  agent_id?: string;
  attempt?: Record<string, unknown>;
  run_id?: string;
  success_count?: number;
  failure_count?: number;
  last_heartbeat?: string;
}

interface LiveState {
  events: DashboardEvent[];
  liveAttempts: Record<string, Record<string, unknown>[]>; // job_id → attempts
  addEvent: (event: DashboardEvent) => void;
  clearEvents: () => void;
}

export const useLiveStore = create<LiveState>((set) => ({
  events: [],
  liveAttempts: {},
  addEvent: (event) =>
    set((state) => {
      const events = [...state.events.slice(-200), event];
      const liveAttempts = { ...state.liveAttempts };

      if (event.type === 'attempt_result' && event.job_id && event.attempt) {
        const jobAttempts = liveAttempts[event.job_id] || [];
        liveAttempts[event.job_id] = [...jobAttempts, event.attempt];
      }

      return { events, liveAttempts };
    }),
  clearEvents: () => set({ events: [], liveAttempts: {} }),
}));
