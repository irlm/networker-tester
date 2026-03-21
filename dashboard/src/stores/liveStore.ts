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
  // Deploy events
  deployment_id?: string;
  line?: string;
  stream?: string;
  endpoint_ips?: string[];
  // Job log events
  level?: string;
}

const MAX_ATTEMPTS_PER_JOB = 2000;
const MAX_DEPLOY_LINES = 5000;
const MAX_JOB_LOGS = 2000;
// Batch rapid attempt_result events to avoid re-rendering on every probe.
// Attempts are buffered and flushed at most every FLUSH_INTERVAL_MS.
const FLUSH_INTERVAL_MS = 500;

interface JobLogLine {
  line: string;
  level: string;
}

interface LiveState {
  events: DashboardEvent[];
  liveAttempts: Record<string, LiveAttempt[]>;
  deployLogs: Record<string, string[]>;
  jobLogs: Record<string, JobLogLine[]>;
  addEvent: (event: DashboardEvent) => void;
  cleanupJob: (jobId: string) => void;
  cleanupDeploy: (deploymentId: string) => void;
  clearEvents: () => void;
}

// Pending attempt buffers (outside Zustand to avoid triggering renders)
const pendingAttempts: Record<string, LiveAttempt[]> = {};
let flushTimer: ReturnType<typeof setTimeout> | null = null;

function scheduleFlush(set: (fn: (state: LiveState) => Partial<LiveState>) => void) {
  if (flushTimer) return; // already scheduled
  flushTimer = setTimeout(() => {
    flushTimer = null;
    // Move pending into store in one batch
    const batch = { ...pendingAttempts };
    for (const k of Object.keys(batch)) delete pendingAttempts[k];
    if (Object.keys(batch).length === 0) return;
    set((state) => {
      const liveAttempts = { ...state.liveAttempts };
      for (const [jobId, newAttempts] of Object.entries(batch)) {
        const existing = liveAttempts[jobId] || [];
        const merged = [...existing, ...newAttempts];
        liveAttempts[jobId] = merged.length > MAX_ATTEMPTS_PER_JOB
          ? merged.slice(-MAX_ATTEMPTS_PER_JOB)
          : merged;
      }
      return { liveAttempts };
    });
  }, FLUSH_INTERVAL_MS);
}

export const useLiveStore = create<LiveState>((set) => ({
  events: [],
  liveAttempts: {},
  deployLogs: {},
  jobLogs: {},
  addEvent: (event) => {
    // Buffer attempt_result events — flush in batches to avoid render thrash
    if (event.type === 'attempt_result' && event.job_id && event.attempt) {
      const jobId = event.job_id;
      if (!pendingAttempts[jobId]) pendingAttempts[jobId] = [];
      pendingAttempts[jobId].push(event.attempt);
      scheduleFlush(set);
      // Still update events array (lightweight, no chart re-render)
      set((state) => ({ events: [...state.events.slice(-200), event] }));
      return;
    }
    // Job completion: flush any pending attempts immediately so the page sees them
    if (event.type === 'job_complete' && event.job_id && pendingAttempts[event.job_id]) {
      if (flushTimer) { clearTimeout(flushTimer); flushTimer = null; }
      const jobId = event.job_id;
      const batch = pendingAttempts[jobId] || [];
      delete pendingAttempts[jobId];
      set((state) => {
        const existing = state.liveAttempts[jobId] || [];
        const merged = [...existing, ...batch];
        return {
          events: [...state.events.slice(-200), event],
          liveAttempts: {
            ...state.liveAttempts,
            [jobId]: merged.length > MAX_ATTEMPTS_PER_JOB ? merged.slice(-MAX_ATTEMPTS_PER_JOB) : merged,
          },
        };
      });
      return;
    }
    set((state) => {
      const events = [...state.events.slice(-200), event];
      if (event.type === 'job_log' && event.job_id && event.line !== undefined) {
        const logs = state.jobLogs[event.job_id] || [];
        const capped = logs.length >= MAX_JOB_LOGS
          ? logs.slice(-MAX_JOB_LOGS + 1)
          : logs;
        return {
          events,
          jobLogs: { ...state.jobLogs, [event.job_id]: [...capped, { line: event.line, level: event.level || 'info' }] },
        };
      }
      if (event.type === 'deploy_log' && event.deployment_id && event.line !== undefined) {
        const lines = state.deployLogs[event.deployment_id] || [];
        const capped = lines.length >= MAX_DEPLOY_LINES
          ? lines.slice(-MAX_DEPLOY_LINES + 1)
          : lines;
        return {
          events,
          deployLogs: { ...state.deployLogs, [event.deployment_id]: [...capped, event.line] },
        };
      }
      return { events };
    });
  },
  cleanupJob: (jobId) =>
    set((state) => {
      const liveAttempts = { ...state.liveAttempts };
      delete liveAttempts[jobId];
      return { liveAttempts };
    }),
  cleanupDeploy: (deploymentId) =>
    set((state) => {
      const deployLogs = { ...state.deployLogs };
      delete deployLogs[deploymentId];
      return { deployLogs };
    }),
  clearEvents: () => set({ events: [], liveAttempts: {}, deployLogs: {}, jobLogs: {} }),
}));
