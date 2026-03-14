const API_BASE = '/api';

async function request<T>(path: string, options?: RequestInit): Promise<T> {
  const token = localStorage.getItem('token');
  const headers: Record<string, string> = {
    'Content-Type': 'application/json',
    ...(token ? { Authorization: `Bearer ${token}` } : {}),
  };

  const res = await fetch(`${API_BASE}${path}`, {
    ...options,
    headers: { ...headers, ...(options?.headers as Record<string, string>) },
  });

  if (res.status === 401) {
    localStorage.removeItem('token');
    window.location.href = '/login';
    throw new Error('Unauthorized');
  }

  if (!res.ok) {
    throw new Error(`API error: ${res.status} ${res.statusText}`);
  }

  return res.json();
}

export const api = {
  login: (username: string, password: string) =>
    request<{ token: string; role: string; username: string }>('/auth/login', {
      method: 'POST',
      body: JSON.stringify({ username, password }),
    }),

  getDashboardSummary: () =>
    request<{
      agents_online: number;
      jobs_running: number;
      runs_24h: number;
      jobs_pending: number;
    }>('/dashboard/summary'),

  getAgents: () =>
    request<{ agents: Agent[] }>('/agents'),

  getJobs: (params?: { status?: string; limit?: number; offset?: number }) => {
    const search = new URLSearchParams();
    if (params?.status) search.set('status', params.status);
    if (params?.limit) search.set('limit', String(params.limit));
    if (params?.offset) search.set('offset', String(params.offset));
    const qs = search.toString();
    return request<Job[]>(`/jobs${qs ? `?${qs}` : ''}`);
  },

  getJob: (jobId: string) => request<Job>(`/jobs/${jobId}`),

  createJob: (config: JobConfig, agentId?: string) =>
    request<{ job_id: string; status: string }>('/jobs', {
      method: 'POST',
      body: JSON.stringify({ config, agent_id: agentId }),
    }),

  cancelJob: (jobId: string) =>
    request<{ status: string }>(`/jobs/${jobId}/cancel`, { method: 'POST' }),

  getRuns: (params?: { target_host?: string; limit?: number; offset?: number }) => {
    const search = new URLSearchParams();
    if (params?.target_host) search.set('target_host', params.target_host);
    if (params?.limit) search.set('limit', String(params.limit));
    if (params?.offset) search.set('offset', String(params.offset));
    const qs = search.toString();
    return request<RunSummary[]>(`/runs${qs ? `?${qs}` : ''}`);
  },

  getRunAttempts: (runId: string) =>
    request<Attempt[]>(`/runs/${runId}/attempts`),
};

// Types
export interface Agent {
  agent_id: string;
  name: string;
  region: string | null;
  provider: string | null;
  status: string;
  version: string | null;
  os: string | null;
  arch: string | null;
  last_heartbeat: string | null;
  registered_at: string;
  tags: Record<string, string> | null;
}

export interface Job {
  job_id: string;
  definition_id: string | null;
  agent_id: string | null;
  status: string;
  config: JobConfig;
  created_by: string | null;
  created_at: string;
  started_at: string | null;
  finished_at: string | null;
  run_id: string | null;
  error_message: string | null;
}

export interface JobConfig {
  target: string;
  modes: string[];
  runs: number;
  concurrency: number;
  timeout_secs: number;
  payload_sizes: string[];
  insecure: boolean;
  dns_enabled: boolean;
  connection_reuse: boolean;
}

export interface RunSummary {
  run_id: string;
  started_at: string;
  finished_at: string | null;
  target_url: string;
  target_host: string;
  modes: string;
  total_runs: number;
  success_count: number;
  failure_count: number;
}

export interface Attempt {
  attempt_id: string;
  protocol: string;
  sequence_num: number;
  started_at: string;
  finished_at: string | null;
  success: boolean;
  error_message: string | null;
  retry_count: number;
}
