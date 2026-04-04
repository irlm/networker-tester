import type { Agent, Job, JobConfig, RunSummary, Attempt, Deployment, CloudStatus, ModeGroup, PacketCaptureSummary, Schedule, DashUser, CloudConnection, CloudAccountSummary, ProjectSummary, ProjectDetail, ProjectMember, ShareLink, CommandApproval, WorkspaceInvite, ResolvedInvite, SystemMetrics, DbMetrics, WorkspaceUsage, LogEntry, BenchmarkRunSummary, BenchmarkArtifact, BenchmarkComparisonReport, BenchmarkComparePreset, BenchmarkComparePresetInput, TlsProfileSummary, TlsProfileDetail, BenchmarkConfigSummary, BenchmarkVmCatalogEntry, BenchTokenInfo } from './types';

export type { Agent, Job, JobConfig, RunSummary, Attempt, Deployment, CloudStatus, ModeGroup, PacketCaptureSummary, Schedule, DashUser, CloudConnection, CloudAccountSummary, ProjectSummary, ProjectDetail, ProjectMember, ShareLink, CommandApproval, WorkspaceInvite, ResolvedInvite, SystemMetrics, DbMetrics, WorkspaceUsage, LogEntry, BenchmarkRunSummary, BenchmarkArtifact, BenchmarkComparisonReport, BenchmarkComparePreset, BenchmarkComparePresetInput, TlsProfileSummary, TlsProfileDetail, BenchmarkConfigSummary, BenchmarkVmCatalogEntry, BenchTokenInfo };
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
    localStorage.removeItem('isPlatformAdmin');
    localStorage.removeItem('activeProjectId');
    localStorage.removeItem('activeProjectSlug');
    localStorage.removeItem('activeProjectRole');
    window.location.href = '/login';
    throw new Error('Unauthorized');
  }

  if (res.status === 403) {
    const body = await res.text();
    if (body === 'pending_approval') {
      // Update stored status and redirect to pending page
      localStorage.setItem('status', 'pending');
      if (window.location.pathname !== '/pending') {
        window.location.href = '/pending';
      }
      throw new Error('pending_approval');
    }
    throw new Error(`API error: ${res.status} ${body}`);
  }

  if (!res.ok) {
    throw new Error(`API error: ${res.status} ${res.statusText}`);
  }

  return res.json();
}

function projectUrl(projectId: string, path: string): string {
  return `/projects/${projectId}/${path}`;
}

export const api = {
  // ── Auth (NOT project-scoped) ─────────────────────────────────────────
  login: (email: string, password: string) =>
    request<{ token: string; role: string; email: string; status: string; must_change_password: boolean; is_platform_admin?: boolean }>('/auth/login', {
      method: 'POST',
      body: JSON.stringify({ email, password }),
    }),

  changePassword: (currentPassword: string, newPassword: string) =>
    request<{ success: boolean }>('/auth/change-password', {
      method: 'POST',
      body: JSON.stringify({ current_password: currentPassword, new_password: newPassword }),
    }),

  getProfile: () =>
    request<{ email: string; role: string; status: string }>('/auth/profile'),

  ssoExchange: (code: string) =>
    request<{ token: string; role: string; email: string; status: string; must_change_password: boolean; is_platform_admin?: boolean }>('/auth/sso/exchange', {
      method: 'POST',
      body: JSON.stringify({ code }),
    }),

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

  // SSO
  getProviders: () =>
    request<{ providers: string[] }>('/auth/sso/providers'),

  checkEmail: (email: string) =>
    request<{ provider: string | null }>('/auth/sso/check-email', {
      method: 'POST',
      body: JSON.stringify({ email }),
    }),

  exchangeCode: (code: string) =>
    fetch(`${API_BASE}/auth/sso/exchange`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ code }),
    }).then(async r => {
      if (!r.ok) throw new Error(await r.text());
      return r.json() as Promise<{ token: string; email: string; role: string; status: string }>;
    }),

  // ── Projects ──────────────────────────────────────────────────────────
  getProjects: () =>
    request<{ projects: ProjectSummary[] }>('/projects').then(d => d.projects),

  createProject: (name: string, description?: string) =>
    request<{ project_id: string; slug: string }>('/projects', {
      method: 'POST',
      body: JSON.stringify({ name, description }),
    }),

  getProject: (projectId: string) =>
    request<ProjectDetail>(`/projects/${projectId}`),

  updateProject: (projectId: string, params: { name?: string; description?: string; settings?: Record<string, unknown> }) =>
    request<{ updated: boolean }>(`/projects/${projectId}`, {
      method: 'PUT',
      body: JSON.stringify(params),
    }),

  deleteProject: (projectId: string) =>
    request<{ deleted: boolean }>(`/projects/${projectId}`, { method: 'DELETE' }),

  getProjectMembers: (projectId: string) =>
    request<{ members: ProjectMember[] }>(`/projects/${projectId}/members`).then(d => d.members),

  addProjectMember: (projectId: string, email: string, role: string) =>
    request<{ user_id: string }>(`/projects/${projectId}/members`, {
      method: 'POST',
      body: JSON.stringify({ email, role }),
    }),

  updateMemberRole: (projectId: string, userId: string, role: string) =>
    request<{ updated: boolean }>(`/projects/${projectId}/members/${userId}`, {
      method: 'PUT',
      body: JSON.stringify({ role }),
    }),

  removeProjectMember: (projectId: string, userId: string) =>
    request<{ removed: boolean }>(`/projects/${projectId}/members/${userId}`, { method: 'DELETE' }),

  // ── Project Invites (project-scoped, admin only) ────────────────────
  getInvites: (projectId: string) =>
    request<WorkspaceInvite[]>(projectUrl(projectId, 'invites')),

  createInvite: (projectId: string, email: string, role: string) =>
    request<{ invite_id: string; url: string; expires_at: string }>(projectUrl(projectId, 'invites'), {
      method: 'POST',
      body: JSON.stringify({ email, role }),
    }),

  revokeInvite: (projectId: string, inviteId: string) =>
    request<{ revoked: boolean }>(projectUrl(projectId, `invites/${inviteId}`), { method: 'DELETE' }).then(() => {}),

  // ── Public invite endpoints (no auth) ───────────────────────────────
  resolveInvite: (token: string) =>
    fetch(`${API_BASE}/invite/${token}`).then(async r => {
      if (!r.ok) throw new Error(await r.text());
      return r.json() as Promise<ResolvedInvite>;
    }),

  acceptInvite: (token: string, password?: string, currentPassword?: string) =>
    fetch(`${API_BASE}/invite/${token}/accept`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        ...(password ? { password } : {}),
        ...(currentPassword ? { current_password: currentPassword } : {}),
      }),
    }).then(async r => {
      if (!r.ok) throw new Error(await r.text());
      return r.json() as Promise<{ token: string; email: string; role: string; project_id: string }>;
    }),

  // ── Project-scoped resources ──────────────────────────────────────────

  getDashboardSummary: (projectId: string) =>
    request<{
      agents_online: number;
      jobs_running: number;
      runs_24h: number;
      jobs_pending: number;
    }>(projectUrl(projectId, 'dashboard/summary')),

  getAgents: (projectId: string) =>
    request<{ agents: Agent[] }>(projectUrl(projectId, 'agents')),

  createAgent: (projectId: string, params: {
    name: string;
    location: 'local' | 'ssh';
    region?: string;
    provider?: string;
    ssh_host?: string;
    ssh_user?: string;
    ssh_port?: number;
  }) =>
    request<{ agent_id: string; api_key: string; name: string; status: string }>(projectUrl(projectId, 'agents'), {
      method: 'POST',
      body: JSON.stringify(params),
    }),

  deleteAgent: (projectId: string, agentId: string) =>
    request<{ deleted: boolean }>(projectUrl(projectId, `agents/${agentId}`), { method: 'DELETE' }),

  deployTesterVm: (projectId: string, params: { name: string; provider: string; region: string; vm_size: string }) =>
    request<{ agent_id: string; status: string }>(projectUrl(projectId, 'agents/deploy-vm'), {
      method: 'POST',
      body: JSON.stringify(params),
    }),

  getJobs: (projectId: string, params?: { status?: string; limit?: number; offset?: number }) => {
    const search = new URLSearchParams();
    if (params?.status) search.set('status', params.status);
    if (params?.limit) search.set('limit', String(params.limit));
    if (params?.offset) search.set('offset', String(params.offset));
    const qs = search.toString();
    return request<Job[]>(projectUrl(projectId, `jobs${qs ? `?${qs}` : ''}`));
  },

  getJob: (projectId: string, jobId: string) => request<Job>(projectUrl(projectId, `jobs/${jobId}`)),

  createJob: (projectId: string, config: JobConfig, agentId?: string) =>
    request<{ job_id: string; status: string }>(projectUrl(projectId, 'jobs'), {
      method: 'POST',
      body: JSON.stringify({ config, agent_id: agentId }),
    }),

  cancelJob: (projectId: string, jobId: string) =>
    request<{ status: string }>(projectUrl(projectId, `jobs/${jobId}/cancel`), { method: 'POST' }),

  getRuns: (projectId: string, params?: { target_host?: string; limit?: number; offset?: number }) => {
    const search = new URLSearchParams();
    if (params?.target_host) search.set('target_host', params.target_host);
    if (params?.limit) search.set('limit', String(params.limit));
    if (params?.offset) search.set('offset', String(params.offset));
    const qs = search.toString();
    return request<RunSummary[]>(projectUrl(projectId, `runs${qs ? `?${qs}` : ''}`));
  },

  getRun: (projectId: string, runId: string) =>
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
    }>(projectUrl(projectId, `runs/${runId}`)),

  getRunAttempts: (projectId: string, runId: string) =>
    request<Attempt[]>(projectUrl(projectId, `runs/${runId}/attempts`)),

  getTlsProfiles: (projectId: string, params?: { limit?: number; offset?: number }) => {
    const search = new URLSearchParams();
    if (params?.limit) search.set('limit', String(params.limit));
    if (params?.offset) search.set('offset', String(params.offset));
    const qs = search.toString();
    return request<TlsProfileSummary[]>(projectUrl(projectId, `tls-profiles${qs ? `?${qs}` : ''}`));
  },

  getTlsProfile: (projectId: string, runId: string) =>
    request<TlsProfileDetail>(projectUrl(projectId, `tls-profiles/${runId}`)),

  getBenchmarks: (projectId: string, params?: { target_host?: string; limit?: number; offset?: number }) => {
    const search = new URLSearchParams();
    if (params?.target_host) search.set('target_host', params.target_host);
    if (params?.limit) search.set('limit', String(params.limit));
    if (params?.offset) search.set('offset', String(params.offset));
    const qs = search.toString();
    return request<BenchmarkRunSummary[]>(projectUrl(projectId, `benchmarks${qs ? `?${qs}` : ''}`));
  },

  getBenchmark: (projectId: string, runId: string) =>
    request<BenchmarkArtifact>(projectUrl(projectId, `benchmarks/${runId}`)),

  compareBenchmarks: (projectId: string, runIds: string[], baselineRunId?: string) =>
    request<BenchmarkComparisonReport>(projectUrl(projectId, 'benchmarks/compare'), {
      method: 'POST',
      body: JSON.stringify({ run_ids: runIds, baseline_run_id: baselineRunId }),
    }),

  getBenchmarkComparePresets: (projectId: string) =>
    request<BenchmarkComparePreset[]>(projectUrl(projectId, 'benchmarks/presets')),

  saveBenchmarkComparePreset: (projectId: string, preset: BenchmarkComparePresetInput) =>
    request<BenchmarkComparePreset[]>(projectUrl(projectId, 'benchmarks/presets'), {
      method: 'POST',
      body: JSON.stringify(preset),
    }),

  deleteBenchmarkComparePreset: (projectId: string, presetId: string) =>
    request<BenchmarkComparePreset[]>(projectUrl(projectId, `benchmarks/presets/${presetId}`), {
      method: 'DELETE',
    }),

  // Deployments
  getDeployments: (projectId: string, params?: { limit?: number; offset?: number }) => {
    const search = new URLSearchParams();
    if (params?.limit) search.set('limit', String(params.limit));
    if (params?.offset) search.set('offset', String(params.offset));
    const qs = search.toString();
    return request<Deployment[]>(projectUrl(projectId, `deployments${qs ? `?${qs}` : ''}`));
  },

  getDeployment: (projectId: string, deploymentId: string) =>
    request<Deployment>(projectUrl(projectId, `deployments/${deploymentId}`)),

  createDeployment: (projectId: string, name: string, config: unknown) =>
    request<{ deployment_id: string; status: string }>(projectUrl(projectId, 'deployments'), {
      method: 'POST',
      body: JSON.stringify({ name, config }),
    }),

  stopDeployment: (projectId: string, deploymentId: string) =>
    request<{ status: string }>(projectUrl(projectId, `deployments/${deploymentId}/stop`), { method: 'POST' }),

  deleteDeployment: (projectId: string, deploymentId: string) =>
    request<{ deleted: boolean }>(projectUrl(projectId, `deployments/${deploymentId}`), { method: 'DELETE' }),

  checkDeployment: (projectId: string, deploymentId: string) =>
    request<{ endpoints: { ip: string; alive: boolean }[] }>(projectUrl(projectId, `deployments/${deploymentId}/check`), { method: 'POST' }),

  updateEndpoint: (projectId: string, deploymentId: string) =>
    request<{ status: string }>(projectUrl(projectId, `deployments/${deploymentId}/update`), { method: 'POST' }),

  // Cloud status
  getCloudStatus: (projectId: string) => request<CloudStatus>(projectUrl(projectId, 'cloud/status')),

  // Modes (NOT project-scoped)
  getModes: () => request<{ groups: ModeGroup[] }>('/modes'),

  // Updates (NOT project-scoped)
  updateLocalTester: () =>
    request<{ status: string; update_id: string }>('/update/tester', { method: 'POST' }),

  updateDashboard: () =>
    request<{ status: string; update_id: string }>('/update/dashboard', { method: 'POST' }),

  // Inventory
  getInventory: (projectId: string) =>
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
    }>(projectUrl(projectId, 'inventory')),

  // Schedules
  getSchedules: (projectId: string) =>
    request<Schedule[]>(projectUrl(projectId, 'schedules')),

  getSchedule: (projectId: string, scheduleId: string) =>
    request<{ schedule: Schedule; recent_jobs: Job[] }>(projectUrl(projectId, `schedules/${scheduleId}`)),

  createSchedule: (projectId: string, params: {
    name: string;
    cron_expr: string;
    config: JobConfig | Record<string, never>;
    agent_id?: string;
    deployment_id?: string;
    auto_start_vm?: boolean;
    auto_stop_vm?: boolean;
    benchmark_config_id?: string;
  }) =>
    request<{ schedule_id: string; next_run_at: string }>(projectUrl(projectId, 'schedules'), {
      method: 'POST',
      body: JSON.stringify(params),
    }),

  updateSchedule: (projectId: string, scheduleId: string, params: {
    name: string;
    cron_expr: string;
    config: JobConfig;
    agent_id?: string;
    deployment_id?: string;
    auto_start_vm?: boolean;
    auto_stop_vm?: boolean;
  }) =>
    request<{ status: string; next_run_at: string }>(projectUrl(projectId, `schedules/${scheduleId}`), {
      method: 'PUT',
      body: JSON.stringify(params),
    }),

  deleteSchedule: (projectId: string, scheduleId: string) =>
    request<{ deleted: boolean }>(projectUrl(projectId, `schedules/${scheduleId}`), { method: 'DELETE' }),

  toggleSchedule: (projectId: string, scheduleId: string) =>
    request<{ enabled: boolean }>(projectUrl(projectId, `schedules/${scheduleId}/toggle`), { method: 'POST' }),

  triggerSchedule: (projectId: string, scheduleId: string) =>
    request<{ job_id: string; status: string }>(projectUrl(projectId, `schedules/${scheduleId}/trigger`), { method: 'POST' }),

  // Users (admin-only, NOT project-scoped)
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

  inviteUser: (email: string, role: string) =>
    request<{ user_id: string }>('/users/invite', {
      method: 'POST',
      body: JSON.stringify({ email, role }),
    }),

  // Cloud Connections
  getCloudConnections: (projectId: string) =>
    request<CloudConnection[]>(projectUrl(projectId, 'cloud-connections')),

  createCloudConnection: (projectId: string, params: { name: string; provider: string; config: unknown }) =>
    request<{ connection_id: string }>(projectUrl(projectId, 'cloud-connections'), {
      method: 'POST',
      body: JSON.stringify(params),
    }),

  updateCloudConnection: (projectId: string, id: string, params: { name: string; config: unknown }) =>
    request<{ updated: boolean }>(projectUrl(projectId, `cloud-connections/${id}`), {
      method: 'PUT',
      body: JSON.stringify(params),
    }),

  deleteCloudConnection: (projectId: string, id: string) =>
    request<{ deleted: boolean }>(projectUrl(projectId, `cloud-connections/${id}`), { method: 'DELETE' }),

  validateCloudConnection: (projectId: string, id: string) =>
    request<{ status: string; validation_error: string | null }>(projectUrl(projectId, `cloud-connections/${id}/validate`), { method: 'POST' }),

  // Cloud Accounts
  getCloudAccounts: (projectId: string) =>
    request<CloudAccountSummary[]>(projectUrl(projectId, 'cloud-accounts')),

  createCloudAccount: (projectId: string, params: { name: string; provider: string; credentials: Record<string, string>; region_default?: string; personal: boolean }) =>
    request<{ account_id: string }>(projectUrl(projectId, 'cloud-accounts'), {
      method: 'POST',
      body: JSON.stringify(params),
    }),

  // Share Links (project-scoped, admin only)
  getShareLinks: (projectId: string) =>
    request<ShareLink[]>(projectUrl(projectId, 'share-links')),

  createShareLink: (projectId: string, params: { resource_type: string; resource_id: string; label?: string; expires_in_days: number }) =>
    request<{ link_id: string; url: string; expires_at: string }>(projectUrl(projectId, 'share-links'), {
      method: 'POST',
      body: JSON.stringify(params),
    }),

  updateCloudAccount: (projectId: string, accountId: string, params: { name: string; region_default?: string }) =>
    request<void>(projectUrl(projectId, `cloud-accounts/${accountId}`), {
      method: 'PUT',
      body: JSON.stringify(params),
    }),

  deleteCloudAccount: (projectId: string, accountId: string) =>
    request<void>(projectUrl(projectId, `cloud-accounts/${accountId}`), { method: 'DELETE' }),

  validateCloudAccount: (projectId: string, accountId: string) =>
    request<{ status: string; validation_error?: string }>(projectUrl(projectId, `cloud-accounts/${accountId}/validate`), { method: 'POST' }),

  extendShareLink: (projectId: string, linkId: string, days: number) =>
    request<void>(projectUrl(projectId, `share-links/${linkId}`), {
      method: 'PUT',
      body: JSON.stringify({ action: 'extend', expires_in_days: days }),
    }),

  revokeShareLink: (projectId: string, linkId: string) =>
    request<void>(projectUrl(projectId, `share-links/${linkId}`), {
      method: 'PUT',
      body: JSON.stringify({ action: 'revoke' }),
    }),

  deleteShareLink: (projectId: string, linkId: string) =>
    request<void>(projectUrl(projectId, `share-links/${linkId}`), { method: 'DELETE' }),

  resolveShareLink: (token: string) =>
    fetch(`${API_BASE}/share/${token}`).then(async r => {
      if (!r.ok) throw new Error(await r.text());
      return r.json();
    }),

  // Visibility Rules (project-scoped, admin only)
  getVisibilityRules: (projectId: string) =>
    request<Record<string, unknown>[]>(projectUrl(projectId, 'visibility-rules')),

  addVisibilityRule: (projectId: string, params: { user_id?: string; resource_type: string; resource_id: string }) =>
    request<{ rule_id: string }>(projectUrl(projectId, 'visibility-rules'), {
      method: 'POST',
      body: JSON.stringify(params),
    }),

  removeVisibilityRule: (projectId: string, ruleId: string) =>
    request<void>(projectUrl(projectId, `visibility-rules/${ruleId}`), { method: 'DELETE' }),

  // Command Approvals (project-scoped, admin only)
  getPendingApprovals: (projectId: string) =>
    request<{ approvals: CommandApproval[] }>(projectUrl(projectId, 'command-approvals')).then(d => d.approvals),

  getPendingApprovalCount: (projectId: string) =>
    request<{ count: number }>(projectUrl(projectId, 'command-approvals/count')).then(d => d.count),

  decideApproval: (projectId: string, approvalId: string, approved: boolean, reason?: string) =>
    request<{ status: string }>(projectUrl(projectId, `command-approvals/${approvalId}`), {
      method: 'POST',
      body: JSON.stringify({ approved, reason }),
    }).then(() => {}),

  // Version (NOT project-scoped)
  getVersionInfo: () => request<{
    dashboard_version: string;
    tester_version: string | null;
    latest_release: string | null;
    update_available: boolean;
    endpoints: { host: string; version: string | null; reachable: boolean }[];
  }>('/version'),

  // ── System Admin (platform admin only, NOT project-scoped) ──────────
  getSystemMetrics: () =>
    request<{ system: SystemMetrics; database: DbMetrics }>('/admin/metrics').then(r => ({ system: r.system, db: r.database })),

  getWorkspaceUsage: () =>
    request<WorkspaceUsage[]>('/admin/workspaces'),

  getSystemLogs: (params?: { level?: string; search?: string; limit?: number }) => {
    const search = new URLSearchParams();
    if (params?.level) search.set('level', params.level);
    if (params?.search) search.set('search', params.search);
    if (params?.limit) search.set('limit', String(params.limit));
    const qs = search.toString();
    return request<LogEntry[]>(`/admin/logs${qs ? `?${qs}` : ''}`);
  },

  suspendWorkspace: (projectId: string) =>
    request<void>(`/admin/workspaces/${projectId}/suspend`, { method: 'POST' }),

  restoreWorkspace: (projectId: string) =>
    request<void>(`/admin/workspaces/${projectId}/restore`, { method: 'POST' }),

  protectWorkspace: (projectId: string) =>
    request<{ delete_protection: boolean }>(`/admin/workspaces/${projectId}/protect`, { method: 'POST' }),

  hardDeleteWorkspace: (projectId: string) =>
    request<void>(`/admin/workspaces/${projectId}`, { method: 'DELETE' }),

  // Leaderboard (simple benchmark routes)
  getLeaderboard: () =>
    request<import('./types').BenchmarkLeaderboardEntry[]>('/leaderboard'),

  getLeaderboardRuns: () =>
    request<import('./types').BenchmarkRun[]>('/leaderboard/runs'),

  getLeaderboardRun: (runId: string) =>
    request<import('./types').BenchmarkRun>(`/leaderboard/runs/${runId}`),

  uploadLeaderboardResults: (payload: { name: string; config?: Record<string, unknown>; results: Array<{ language: string; runtime: string; metrics?: Record<string, number>; server_os?: string; client_os?: string; cloud?: string; phase?: string; concurrency?: number }> }) =>
    request<import('./types').BenchmarkRun>('/leaderboard/upload', {
      method: 'POST',
      body: JSON.stringify(payload),
    }),

  // ── Benchmark VM Catalog ──
  listBenchmarkCatalog: (projectId: string) =>
    request<import('./types').BenchmarkVmCatalogEntry[]>(projectUrl(projectId, 'benchmark-catalog')),

  registerBenchmarkVm: (projectId: string, payload: { name: string; ip: string; ssh_user: string; cloud: string; region: string }) =>
    request<import('./types').BenchmarkVmCatalogEntry>(projectUrl(projectId, 'benchmark-catalog'), {
      method: 'POST',
      body: JSON.stringify(payload),
    }),

  deleteBenchmarkVm: (projectId: string, vmId: string) =>
    request<{ deleted: boolean }>(projectUrl(projectId, `benchmark-catalog/${vmId}`), { method: 'DELETE' }),

  detectBenchmarkVmLanguages: (projectId: string, vmId: string) =>
    request<{ languages: string[] }>(projectUrl(projectId, `benchmark-catalog/${vmId}/detect`), { method: 'POST' }),

  // ── Benchmark Configs (wizard) ────────────────────────────────────────
  listBenchmarkConfigs: (projectId: string) =>
    request<BenchmarkConfigSummary[]>(projectUrl(projectId, 'benchmark-configs')),

  getBenchmarkConfig: (projectId: string, configId: string) =>
    request<BenchmarkConfigSummary>(projectUrl(projectId, `benchmark-configs/${configId}`)),

  createBenchmarkConfig: (projectId: string, payload: { name: string; template: string | null; testbeds: import('./types').BenchmarkTestbedConfig[]; languages: string[]; methodology: Record<string, unknown>; auto_teardown: boolean; benchmark_type?: string }) =>
    request<{ config_id: string }>(projectUrl(projectId, 'benchmark-configs'), {
      method: 'POST',
      body: JSON.stringify(payload),
    }),

  launchBenchmarkConfig: (projectId: string, configId: string) =>
    request<{ status: string }>(projectUrl(projectId, `benchmark-configs/${configId}/launch`), { method: 'POST' }),

  cancelBenchmarkConfig: (projectId: string, configId: string) =>
    request<{ status: string }>(projectUrl(projectId, `benchmark-configs/${configId}/cancel`), { method: 'POST' }),

  getBenchmarkConfigResults: (projectId: string, configId: string) =>
    request<import('./types').BenchmarkConfigResults>(projectUrl(projectId, `benchmark-configs/${configId}/results`)),

  getBenchmarkProgress: (projectId: string, configId: string) =>
    request<{ progress: import('./types').BenchmarkLanguageProgress[] }>(
      projectUrl(projectId, `benchmark-configs/${configId}/progress`)
    ),

  getGroupedLeaderboard: async (group?: string): Promise<import('./types').GroupedLeaderboard> => {
    const params = group ? `?group=${encodeURIComponent(group)}` : '';
    return request<import('./types').GroupedLeaderboard>(`/leaderboard/grouped${params}`);
  },

  // ── Benchmark Regressions ──────────────────────────────────────────
  listBenchmarkRegressions: (projectId: string, limit?: number) =>
    request<import('./types').BenchmarkRegressionWithConfig[]>(
      projectUrl(projectId, `benchmark-regressions${limit ? `?limit=${limit}` : ''}`)
    ),

  getBenchmarkConfigRegressions: (projectId: string, configId: string) =>
    request<import('./types').BenchmarkRegression[]>(
      projectUrl(projectId, `benchmark-configs/${configId}/regressions`)
    ),

  // ── Benchmark Tokens (platform admin only, NOT project-scoped) ──────
  listBenchTokens: () =>
    request<BenchTokenInfo[]>('/bench-tokens'),

  revokeBenchToken: (name: string) =>
    request<{ deleted: boolean }>(`/bench-tokens/${encodeURIComponent(name)}`, { method: 'DELETE' }),

  revokeAllBenchTokens: () =>
    request<{ deleted: number }>('/bench-tokens', { method: 'DELETE' }),
};
