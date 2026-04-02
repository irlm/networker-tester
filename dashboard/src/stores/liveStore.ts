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
  // Benchmark events
  config_id?: string;
  event_type?: string;
  payload?: Record<string, unknown>;
}

const MAX_ATTEMPTS_PER_JOB = 2000;
const MAX_DEPLOY_LINES = 5000;
const MAX_JOB_LOGS = 2000;
const MAX_BENCHMARK_LOGS = 5000;
// Batch rapid attempt_result events to avoid re-rendering on every probe.
// Attempts are buffered and flushed at most every FLUSH_INTERVAL_MS.
const FLUSH_INTERVAL_MS = 500;

interface JobLogLine {
  line: string;
  level: string;
}

export interface BenchmarkTestbedStatus {
  testbed_id: string;
  status: string;
  current_language: string | null;
  language_index: number | null;
  language_total: number | null;
}

export interface BenchmarkResult {
  language: string;
  run_id: string;
  artifact: Record<string, unknown>;
}

export interface BenchmarkLive {
  logs: string[];
  testbeds: Record<string, BenchmarkTestbedStatus>;
  results: BenchmarkResult[];
  configStatus: string | null;
  completedAt: number | null;
  errorMessage: string | null;
}

interface LiveState {
  events: DashboardEvent[];
  liveAttempts: Record<string, LiveAttempt[]>;
  deployLogs: Record<string, string[]>;
  jobLogs: Record<string, JobLogLine[]>;
  benchmarks: Record<string, BenchmarkLive>;
  addEvent: (event: DashboardEvent) => void;
  cleanupJob: (jobId: string) => void;
  cleanupDeploy: (deploymentId: string) => void;
  cleanupBenchmark: (configId: string) => void;
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

function emptyBenchmarkLive(): BenchmarkLive {
  return { logs: [], testbeds: {}, results: [], configStatus: null, completedAt: null, errorMessage: null };
}

export const useLiveStore = create<LiveState>((set) => ({
  events: [],
  liveAttempts: {},
  deployLogs: {},
  jobLogs: {},
  benchmarks: {},
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
      // Benchmark update events
      if (event.type === 'benchmark_update' && event.config_id && event.event_type && event.payload) {
        const cid = event.config_id;
        const existing = state.benchmarks[cid] || emptyBenchmarkLive();
        const updated = { ...existing };

        if (event.event_type === 'log') {
          const newLines = (event.payload.lines as string[]) || [];
          const merged = [...existing.logs, ...newLines];
          updated.logs = merged.length > MAX_BENCHMARK_LOGS ? merged.slice(-MAX_BENCHMARK_LOGS) : merged;
        } else if (event.event_type === 'status') {
          const testbedId = event.payload.testbed_id as string | null;
          if (testbedId) {
            updated.testbeds = {
              ...existing.testbeds,
              [testbedId]: {
                testbed_id: testbedId,
                status: (event.payload.status as string) || 'unknown',
                current_language: (event.payload.current_language as string | null) ?? null,
                language_index: (event.payload.language_index as number | null) ?? null,
                language_total: (event.payload.language_total as number | null) ?? null,
              },
            };
          } else {
            updated.configStatus = (event.payload.status as string) || null;
          }
        } else if (event.event_type === 'result') {
          updated.results = [
            ...existing.results,
            {
              language: (event.payload.language as string) || 'unknown',
              run_id: (event.payload.run_id as string) || '',
              artifact: (event.payload.artifact as Record<string, unknown>) || {},
            },
          ];
        } else if (event.event_type === 'complete') {
          updated.configStatus = (event.payload.status as string) || 'completed';
          updated.completedAt = Date.now();
          updated.errorMessage = (event.payload.error_message as string | null) ?? null;
        }

        return {
          events,
          benchmarks: { ...state.benchmarks, [cid]: updated },
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
  cleanupBenchmark: (configId) =>
    set((state) => {
      const benchmarks = { ...state.benchmarks };
      delete benchmarks[configId];
      return { benchmarks };
    }),
  clearEvents: () => set({ events: [], liveAttempts: {}, deployLogs: {}, jobLogs: {}, benchmarks: {} }),
}));
