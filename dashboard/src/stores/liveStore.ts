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

export const useLiveStore = create<LiveState>((set) => ({
  events: [],
  liveAttempts: {},
  deployLogs: {},
  jobLogs: {},
  addEvent: (event) =>
    set((state) => {
      const events = [...state.events.slice(-200), event];
      if (event.type === 'attempt_result' && event.job_id && event.attempt) {
        const jobAttempts = state.liveAttempts[event.job_id] || [];
        const capped = jobAttempts.length >= MAX_ATTEMPTS_PER_JOB
          ? jobAttempts.slice(-MAX_ATTEMPTS_PER_JOB + 1)
          : jobAttempts;
        return {
          events,
          liveAttempts: { ...state.liveAttempts, [event.job_id]: [...capped, event.attempt] },
        };
      }
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
    }),
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
