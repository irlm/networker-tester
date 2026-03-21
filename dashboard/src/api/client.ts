import type { Agent, Job, JobConfig, RunSummary, Attempt, Deployment, CloudStatus, ModeGroup, PacketCaptureSummary, Schedule, DashUser } from './types';

export type { Agent, Job, JobConfig, RunSummary, Attempt, Deployment, CloudStatus, ModeGroup, PacketCaptureSummary, Schedule, DashUser };
export type { LiveAttempt } from './types';

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
    localStorage.removeItem('email');
    localStorage.removeItem('role');
    localStorage.removeItem('status');
    localStorage.removeItem('mustChangePassword');
    window.location.href = '/login';
    throw new Error('Unauthorized');
  }

  if (!res.ok) {
    throw new Error(`API error: ${res.status} ${res.statusText}`);
  }

  return res.json();
}

export const api = {
  login: (email: string, password: string) =>
    request<{ token: string; role: string; email: string; status: string; must_change_password: boolean }>('/auth/login', {
      method: 'POST',
      body: JSON.stringify({ email, password }),
    }),

  changePassword: (currentPassword: string, newPassword: string) =>
    request<{ success: boolean }>('/auth/change-password', {
      method: 'POST',
      body: JSON.stringify({ current_password: currentPassword, new_password: newPassword }),
    }),

  getProfile: () =>
    request<{ email: string; role: string }>('/auth/profile'),

  forgotPassword: (email: string) =>
    fetch(`${API_BASE}/auth/forgot-password`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ email }),
    }).then(r => r.json()) as Promise<{ sent: boolean }>,

  resetPassword: (token: string, newPassword: string) =>
    fetch(`${API_BASE}/auth/reset-password`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ token, new_password: newPassword }),
    }).then(async r => {
      if (!r.ok) throw new Error(await r.text());
      return r.json();
    }) as Promise<{ success: boolean }>,

  getDashboardSummary: () =>
    request<{
      agents_online: number;
      jobs_running: number;
      runs_24h: number;
      jobs_pending: number;
    }>('/dashboard/summary'),

  getAgents: () =>
    request<{ agents: Agent[] }>('/agents'),

  createAgent: (params: {
    name: string;
    location: 'local' | 'ssh';
    region?: string;
    provider?: string;
    ssh_host?: string;
    ssh_user?: string;
    ssh_port?: number;
  }) =>
    request<{ agent_id: string; api_key: string; name: string; status: string }>('/agents', {
      method: 'POST',
      body: JSON.stringify(params),
    }),

  deleteAgent: (agentId: string) =>
    request<{ deleted: boolean }>(`/agents/${agentId}`, { method: 'DELETE' }),

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

  getRun: (runId: string) =>
    request<{
      run_id: string;
      target_url: string;
      target_host: string;
      modes: string;
      client_os: string;
      client_version: string;
      endpoint_version: string | null;
      success_count: number;
      failure_count: number;
      packet_capture: PacketCaptureSummary | null;
    }>(`/runs/${runId}`),

  getRunAttempts: (runId: string) =>
    request<Attempt[]>(`/runs/${runId}/attempts`),

  // Deployments
  getDeployments: (params?: { limit?: number; offset?: number }) => {
    const search = new URLSearchParams();
    if (params?.limit) search.set('limit', String(params.limit));
    if (params?.offset) search.set('offset', String(params.offset));
    const qs = search.toString();
    return request<Deployment[]>(`/deployments${qs ? `?${qs}` : ''}`);
  },

  getDeployment: (deploymentId: string) =>
    request<Deployment>(`/deployments/${deploymentId}`),

  createDeployment: (name: string, config: unknown) =>
    request<{ deployment_id: string; status: string }>('/deployments', {
      method: 'POST',
      body: JSON.stringify({ name, config }),
    }),

  stopDeployment: (deploymentId: string) =>
    request<{ status: string }>(`/deployments/${deploymentId}/stop`, { method: 'POST' }),

  deleteDeployment: (deploymentId: string) =>
    request<{ deleted: boolean }>(`/deployments/${deploymentId}`, { method: 'DELETE' }),

  checkDeployment: (deploymentId: string) =>
    request<{ endpoints: { ip: string; alive: boolean }[] }>(`/deployments/${deploymentId}/check`, { method: 'POST' }),

  updateEndpoint: (deploymentId: string) =>
    request<{ status: string }>(`/deployments/${deploymentId}/update`, { method: 'POST' }),

  // Cloud status
  getCloudStatus: () => request<CloudStatus>('/cloud/status'),

  // Modes
  getModes: () => request<{ groups: ModeGroup[] }>('/modes'),

  // Updates
  updateLocalTester: () =>
    request<{ status: string; update_id: string }>('/update/tester', { method: 'POST' }),

  updateDashboard: () =>
    request<{ status: string; update_id: string }>('/update/dashboard', { method: 'POST' }),

  // Inventory
  getInventory: () =>
    request<{
      vms: {
        provider: string;
        name: string;
        region: string;
        status: string;
        public_ip: string | null;
        fqdn: string | null;
        vm_size: string | null;
        os: string | null;
        resource_group: string | null;
        managed: boolean;
      }[];
      errors: string[];
    }>('/inventory'),

  // Schedules
  getSchedules: () =>
    request<Schedule[]>('/schedules'),

  getSchedule: (scheduleId: string) =>
    request<{ schedule: Schedule; recent_jobs: Job[] }>(`/schedules/${scheduleId}`),

  createSchedule: (params: {
    name: string;
    cron_expr: string;
    config: JobConfig;
    agent_id?: string;
    deployment_id?: string;
    auto_start_vm?: boolean;
    auto_stop_vm?: boolean;
  }) =>
    request<{ schedule_id: string; next_run_at: string }>('/schedules', {
      method: 'POST',
      body: JSON.stringify(params),
    }),

  updateSchedule: (scheduleId: string, params: {
    name: string;
    cron_expr: string;
    config: JobConfig;
    agent_id?: string;
    deployment_id?: string;
    auto_start_vm?: boolean;
    auto_stop_vm?: boolean;
  }) =>
    request<{ status: string; next_run_at: string }>(`/schedules/${scheduleId}`, {
      method: 'PUT',
      body: JSON.stringify(params),
    }),

  deleteSchedule: (scheduleId: string) =>
    request<{ deleted: boolean }>(`/schedules/${scheduleId}`, { method: 'DELETE' }),

  toggleSchedule: (scheduleId: string) =>
    request<{ enabled: boolean }>(`/schedules/${scheduleId}/toggle`, { method: 'POST' }),

  triggerSchedule: (scheduleId: string) =>
    request<{ job_id: string; status: string }>(`/schedules/${scheduleId}/trigger`, { method: 'POST' }),

  // Users (admin-only)
  getUsers: () =>
    request<DashUser[]>('/users'),

  getPendingUsers: () =>
    request<{ users: DashUser[]; count: number }>('/users/pending'),

  approveUser: (userId: string, role: string) =>
    request<{ approved: boolean }>(`/users/${userId}/approve`, {
      method: 'POST',
      body: JSON.stringify({ role }),
    }),

  denyUser: (userId: string) =>
    request<{ denied: boolean }>(`/users/${userId}/deny`, { method: 'POST' }),

  setUserRole: (userId: string, role: string) =>
    request<{ updated: boolean }>(`/users/${userId}/role`, {
      method: 'PUT',
      body: JSON.stringify({ role }),
    }),

  disableUser: (userId: string) =>
    request<{ disabled: boolean }>(`/users/${userId}/disable`, { method: 'POST' }),

  // Version
  getVersionInfo: () => request<{
    dashboard_version: string;
    tester_version: string | null;
    latest_release: string | null;
    update_available: boolean;
    endpoints: { host: string; version: string | null; reachable: boolean }[];
  }>('/version'),
};
