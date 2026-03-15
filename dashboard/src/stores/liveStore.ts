import { create } from 'zustand';
import type { LiveAttempt } from '../api/types';

export interface DashboardEvent {
  type: string;
  job_id?: string;
  status?: string;
  agent_id?: string;
  attempt?: LiveAttempt;
  run_id?: string;
  success_count?: number;
  failure_count?: number;
  last_heartbeat?: string;
}

const MAX_ATTEMPTS_PER_JOB = 2000;

interface LiveState {
  events: DashboardEvent[];
  liveAttempts: Record<string, LiveAttempt[]>;
  addEvent: (event: DashboardEvent) => void;
  cleanupJob: (jobId: string) => void;
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
        const capped = jobAttempts.length >= MAX_ATTEMPTS_PER_JOB
          ? jobAttempts.slice(-MAX_ATTEMPTS_PER_JOB + 1)
          : jobAttempts;
        liveAttempts[event.job_id] = [...capped, event.attempt];
      }

      return { events, liveAttempts };
    }),
  cleanupJob: (jobId) =>
    set((state) => {
      const liveAttempts = { ...state.liveAttempts };
      delete liveAttempts[jobId];
      return { liveAttempts };
    }),
  clearEvents: () => set({ events: [], liveAttempts: {} }),
}));
