import type { Agent, Job, JobConfig, Attempt, Deployment, DeploymentCostEstimate, TargetCapabilitiesResponse, ModeGroup, PacketCaptureSummary, DashUser, CloudConnection, CloudAccountSummary, ProjectSummary, ProjectDetail, ProjectMember, ShareLink, CommandApproval, WorkspaceInvite, ResolvedInvite, SystemMetrics, DbMetrics, WorkspaceUsage, LogEntry, LogsResponse, BenchmarkRunSummary, BenchmarkArtifact, BenchmarkComparisonReport, TlsProfileSummary, TlsProfileDetail, BenchmarkConfigSummary, BenchmarkVmCatalogEntry, BenchTokenInfo, ImportResult, SendInviteResult, TestConfig, TestConfigListItem, TestConfigCreate, TestRun, TestSchedule, ComparisonReport, ComparisonGroup, ComparisonGroupCreate, AlertChannel, AlertChannelCreate, AlertRule, AlertRuleCreate, AlertEvent, RunGeoInfo, RunClockSync, RunLoadSample } from './types';

export type { Agent, Job, JobConfig, Attempt, Deployment, ModeGroup, PacketCaptureSummary, DashUser, CloudConnection, CloudAccountSummary, ProjectSummary, ProjectDetail, ProjectMember, ShareLink, CommandApproval, WorkspaceInvite, ResolvedInvite, SystemMetrics, DbMetrics, WorkspaceUsage, LogEntry, LogsResponse, BenchmarkRunSummary, BenchmarkArtifact, BenchmarkComparisonReport, TlsProfileSummary, TlsProfileDetail, BenchmarkConfigSummary, BenchmarkVmCatalogEntry, BenchTokenInfo, ImportResult, SendInviteResult, TestConfig, TestConfigListItem, TestConfigCreate, TestRun, TestSchedule, ComparisonReport, ComparisonGroup, ComparisonGroupCreate, AlertChannel, AlertChannelCreate, AlertRule, AlertRuleCreate, AlertEvent };
export type { AlertMetric, AlertComparator, AlertChannelKind, AlertChannelConfig } from './types';
export type { SdkEndpoint, SdkEndpointCreate, AppNetworkReport, AppNetworkGroup, AppNetworkFormulas, AppNetworkVerdict } from './types';
export type { LiveAttempt, EndpointRef, EndpointKind, Workload, Methodology, RunStatus, CaptureMode, OutlierPolicy, QualityGates, PublicationGates, ComparisonCell } from './types';

import { ApiError, request } from './http';
export { ApiError, clearSession, downloadExport, errorMessage, friendlyHttpError, handleUnauthorized, request } from './http';

function projectUrl(projectId: string, path: string): string {
  return `/projects/${projectId}/${path}`;
}

// ── SSO types ──────────────────────────────────────────────────────────────

export interface SsoProviderInfo {
  id: string;
  name: string;
  type: string;
}

export interface SsoProvider {
  provider_id: string;
  name: string;
  provider_type: string;
  client_id: string;
  has_client_secret: boolean;
  issuer_url: string | null;
  tenant_id: string | null;
  extra_config: Record<string, unknown>;
  enabled: boolean;
  display_order: number;
}

export interface CreateSsoProvider {
  name: string;
  provider_type: string;
  client_id: string;
  client_secret: string;
  issuer_url?: string;
  tenant_id?: string;
  enabled?: boolean;
  display_order?: number;
}

export interface PendingProject {
  project_id: string;
  project_name: string;
  role: string;
  invited_by_email: string | null;
  invited_at: string;
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
    request<{ sent: boolean }>('/auth/forgot-password', {
      method: 'POST',
      body: JSON.stringify({ email }),
    }),

  resetPassword: (token: string, newPassword: string) =>
    request<{ success: boolean }>('/auth/reset-password', {
      method: 'POST',
      body: JSON.stringify({ token, new_password: newPassword }),
    }),

  // SSO
  getProviders: () =>
    request<{ providers: SsoProviderInfo[] }>('/auth/sso/providers'),

  // DEAD ENDPOINT — `/auth/sso/check-email` is not served by any backend
  // (neither the Rust dashboard nor the C# control plane ever mounted it).
  // Every call 404'd and LoginPage fell back to the password form via its
  // catch block. Resolve locally so the login flow skips the guaranteed-404
  // round-trip. If per-email SSO routing ships, restore the POST here.
  checkEmail: (_email: string) =>
    Promise.resolve<{ provider: string | null }>({ provider: null }),

  // ── Pending project invitations ──────────────────────────────────────
  getPendingProjects: () =>
    request<{ pending: PendingProject[] }>('/me/pending-projects'),

  acceptProject: (projectId: string) =>
    request<void>(`/projects/${projectId}/members/me/accept`, { method: 'PUT' }),

  denyProject: (projectId: string) =>
    request<void>(`/projects/${projectId}/members/me/deny`, { method: 'PUT' }),

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

  importMembers: (projectId: string, file: File) => {
    const fd = new FormData();
    fd.append('file', file);
    return request<ImportResult>(`/projects/${projectId}/members/import`, {
      method: 'POST',
      body: fd,
    });
  },

  sendInvites: (projectId: string, userIds: string[]) =>
    request<SendInviteResult>(`/projects/${projectId}/members/send-invites`, {
      method: 'POST',
      body: JSON.stringify({ user_ids: userIds }),
    }),

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
    request<ResolvedInvite>(`/invite/${token}`),

  acceptInvite: (token: string, password?: string, currentPassword?: string) =>
    request<{ token: string; email: string; role: string; project_id: string }>(`/invite/${token}/accept`, {
      method: 'POST',
      body: JSON.stringify({
        ...(password ? { password } : {}),
        ...(currentPassword ? { current_password: currentPassword } : {}),
      }),
    }),

  // ── Project-scoped resources ──────────────────────────────────────────

  getDashboardSummary: (projectId: string) =>
    request<{
      agents_online: number;
      jobs_running: number;
      runs_24h: number;
      jobs_pending: number;
    }>(projectUrl(projectId, 'dashboard/summary')),

  // The C# control plane returns a bare array here; the legacy shape wrapped
  // it in `{ agents }`. Accept both so contract drift can never blank the
  // dashboard again (same defensive pattern as getTestRunAttempts below).
  getAgents: (projectId: string) =>
    request<{ agents: Agent[] } | Agent[]>(projectUrl(projectId, 'agents')).then((r) =>
      Array.isArray(r) ? r : (r?.agents ?? [])
    ),


  createJob: (projectId: string, config: JobConfig, agentId?: string) =>
    request<{ job_id: string; status: string }>(projectUrl(projectId, 'jobs'), {
      method: 'POST',
      body: JSON.stringify({ config, agent_id: agentId }),
    }),


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
      // Measurement-depth run envelope (v0.28.78) — data-gated on the client:
      // rendered only when the backend passes them through from the tester
      // TestRun JSON (absent on older backends/runs).
      client_geo?: RunGeoInfo | null;
      target_geo?: RunGeoInfo | null;
      clock_sync?: RunClockSync | null;
      client_load_before?: RunLoadSample | null;
      client_load_after?: RunLoadSample | null;
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

  getDeploymentCostEstimate: (projectId: string, deploymentId: string) =>
    request<DeploymentCostEstimate>(projectUrl(projectId, `deployments/${deploymentId}/cost_estimate`)),

  // Live per-target test support: each endpoint host's /health `services`
  // self-report mapped onto probe modes. supported_modes is null when the
  // host is unreachable or runs a pre-0.28.202 endpoint — fall back to the
  // config-derived summary, never fabricate.
  getDeploymentCapabilities: (projectId: string, deploymentId: string) =>
    request<TargetCapabilitiesResponse>(projectUrl(projectId, `deployments/${deploymentId}/capabilities`)),

  createDeployment: (projectId: string, name: string, config: unknown) =>
    request<{ deployment_id: string; status: string }>(projectUrl(projectId, 'deployments'), {
      method: 'POST',
      body: JSON.stringify({ name, config }),
    }),

  stopDeployment: (projectId: string, deploymentId: string) =>
    request<{ status: string }>(projectUrl(projectId, `deployments/${deploymentId}/stop`), { method: 'POST' }),

  startDeployment: (projectId: string, deploymentId: string) =>
    request<{ status: string; deployment_id: string }>(projectUrl(projectId, `deployments/${deploymentId}/start`), { method: 'POST' }),

  deleteDeployment: (projectId: string, deploymentId: string) =>
    request<{ deleted: boolean }>(projectUrl(projectId, `deployments/${deploymentId}`), { method: 'DELETE' }),

  checkDeployment: (projectId: string, deploymentId: string) =>
    request<{ endpoints: { ip: string; alive: boolean }[] }>(projectUrl(projectId, `deployments/${deploymentId}/check`), { method: 'POST' }),

  updateEndpoint: (projectId: string, deploymentId: string) =>
    request<{ status: string }>(projectUrl(projectId, `deployments/${deploymentId}/update`), { method: 'POST' }),

  // Modes (NOT project-scoped). language_capabilities is optional so the UI
  // degrades gracefully against control planes that predate the matrix.
  getModes: () =>
    request<{
      groups: ModeGroup[];
      language_capabilities?: import('./types').LanguageCapability[];
      /** shared/http-stacks.json (v0.28.208+): per-stack h3 + the modes that need it. */
      stacks?: { id: string; http_port: number; https_port: number; h3: boolean }[];
      h3_modes?: string[];
    }>('/modes'),
  // The canonical HTTP-stack manifest the mode⇄target h3 gate enforces
  // (same table as /modes `stacks`, with installer notes).
  getHttpStacks: () =>
    request<{ h3_modes: string[]; stacks: { id: string; http_port: number; https_port: number; h3: boolean; installer?: string | null }[] }>('/http-stacks'),

  // Updates (NOT project-scoped)
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

  updateCloudAccount: (projectId: string, accountId: string, params: { name: string; region_default?: string; credentials?: Record<string, string> }) =>
    request<void>(projectUrl(projectId, `cloud-accounts/${accountId}`), {
      method: 'PUT',
      body: JSON.stringify(params),
    }),

  deleteCloudAccount: (projectId: string, accountId: string) =>
    request<void>(projectUrl(projectId, `cloud-accounts/${accountId}`), { method: 'DELETE' }),

  validateCloudAccount: (projectId: string, accountId: string) =>
    request<{ status: string; validation_error?: string }>(projectUrl(projectId, `cloud-accounts/${accountId}/validate`), { method: 'POST' }),

  cleanCloudAccountOrphans: (projectId: string, accountId: string) =>
    request<{
      orphans_found: number;
      deleted: Array<{ resource_id: string; name: string; kind: string }>;
      failed: Array<{ resource_id: string; name: string; kind: string; error: string }>;
    }>(projectUrl(projectId, `cloud-accounts/${accountId}/clean-orphans`), { method: 'POST' }),

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
    request<{
      resource_type: string;
      resource_id: string | null;
      label: string | null;
      data: unknown;
      shared_by: string;
      expires_at: string;
    }>(`/share/${token}`),

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
    /** Feature-flagged Docker (local) provider (DASHBOARD_DOCKER_PROVIDER=1). */
    docker_provider?: boolean;
  }>('/version'),

  // ── System Admin (platform admin only, NOT project-scoped) ──────────
  getSystemMetrics: () =>
    request<{ system: SystemMetrics; database: DbMetrics }>('/admin/metrics').then(r => ({ system: r.system, db: r.database })),

  getSystemHealth: () =>
    request<{
      live: { core_db: boolean; logs_db: boolean };
      checks: {
        check_name: string;
        status: string;
        value: string | null;
        message: string | null;
        checked_at: string;
      }[];
    }>('/system/health'),

  getWorkspaceUsage: () =>
    request<WorkspaceUsage[]>('/admin/workspaces'),

  getSystemLogs: (params?: { level?: string; service?: string; search?: string; limit?: number; config_id?: string }) => {
    const search = new URLSearchParams();
    if (params?.level) search.set('level', params.level);
    if (params?.service) search.set('service', params.service);
    if (params?.search) search.set('search', params.search);
    if (params?.limit) search.set('limit', String(params.limit));
    if (params?.config_id) search.set('config_id', params.config_id);
    const qs = search.toString();
    return request<LogsResponse>(`/logs${qs ? `?${qs}` : ''}`);
  },

  suspendWorkspace: (projectId: string) =>
    request<void>(`/admin/workspaces/${projectId}/suspend`, { method: 'POST' }),

  restoreWorkspace: (projectId: string) =>
    request<void>(`/admin/workspaces/${projectId}/restore`, { method: 'POST' }),

  protectWorkspace: (projectId: string) =>
    request<{ delete_protection: boolean }>(`/admin/workspaces/${projectId}/protect`, { method: 'POST' }),

  hardDeleteWorkspace: (projectId: string) =>
    request<void>(`/admin/workspaces/${projectId}`, { method: 'DELETE' }),

  // SSO provider admin CRUD.
  // NOTE: the C# control plane mounts these at /api/sso-providers (the old
  // Rust dashboard used /api/admin/sso-providers — the client previously
  // pointed there, which 404'd after the cutover).
  getSsoProviders: () =>
    request<SsoProvider[]>('/sso-providers'),

  createSsoProvider: (data: CreateSsoProvider) =>
    request<SsoProvider>('/sso-providers', { method: 'POST', body: JSON.stringify(data) }),

  updateSsoProvider: (id: string, data: Partial<CreateSsoProvider>) =>
    request<SsoProvider>(`/sso-providers/${id}`, { method: 'PUT', body: JSON.stringify(data) }),

  deleteSsoProvider: (id: string) =>
    request<void>(`/sso-providers/${id}`, { method: 'DELETE' }),

  // System config
  getSystemConfig: (key: string) =>
    // A missing config key is a 404 by design (Rust-parity KV read) and means
    // "not set yet" → null. Only swallow that case; a real failure (401/5xx)
    // must surface instead of masquerading as an empty/unset value.
    request<{ key: string; value: string }>(`/admin/system-config/${key}`).catch((e) => {
      if (e instanceof ApiError && e.status === 404) return null;
      throw e;
    }),

  setSystemConfig: (key: string, value: string) =>
    request<void>(`/admin/system-config/${key}`, { method: 'PUT', body: JSON.stringify({ value }) }),

  // ── Prod run-execution canary (platform admin only) ─────────────────
  getCanaryStatus: () =>
    request<{ configured: boolean; owner: string; repo: string; workflow: string; actions_url: string }>(
      '/admin/canary',
    ),

  dispatchCanary: (inputs: {
    reuse_runner?: boolean;
    apibench?: boolean;
    mode_coverage?: boolean;
    matrix_flow?: boolean;
    windows?: boolean;
    ref?: string;
  }) =>
    request<{ status: string; dispatch_id?: string | null; actions_url: string }>('/admin/canary/dispatch', {
      method: 'POST',
      body: JSON.stringify(inputs),
    }),

  // Durable in-product dispatch history (our DB — works when GitHub is down).
  getCanaryHistory: (limit = 30) =>
    request<{
      items: {
        id: string;
        requested_by: string | null;
        requested_at: string;
        git_ref: string;
        inputs: Record<string, string>;
        run_id: number | null;
        run_url: string | null;
        run_status: string | null;
        conclusion: string | null;
        updated_at: string;
      }[];
    }>(`/admin/canary/history?limit=${limit}`),

  // Live recent soak-canary.yml runs straight from GitHub (includes runs
  // triggered outside the product). Empty + detail when no token/unreachable.
  getCanaryRuns: (limit = 30) =>
    request<{
      configured: boolean;
      detail?: string;
      runs: {
        id: number;
        runNumber: number;
        event: string | null;
        status: string | null;
        conclusion: string | null;
        branch: string | null;
        title: string | null;
        actor: string | null;
        htmlUrl: string | null;
        createdAt: string | null;
        updatedAt: string | null;
      }[];
    }>(`/admin/canary/runs?limit=${limit}`),

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
  getBenchmarkConfigResults: (projectId: string, configId: string) =>
    request<import('./types').BenchmarkConfigResults>(projectUrl(projectId, `benchmark-configs/${configId}/results`)),

  getGroupedLeaderboard: async (group?: string): Promise<import('./types').GroupedLeaderboard> => {
    const params = group ? `?group=${encodeURIComponent(group)}` : '';
    return request<import('./types').GroupedLeaderboard>(`/leaderboard/grouped${params}`);
  },

  // ── Perf-per-cost report ───────────────────────────────────────────
  getPerfPerCostReport: (projectId: string) =>
    request<import('./types').PerfPerCostReport>(
      projectUrl(projectId, 'reports/perf-per-cost')
    ),

  // ── Benchmark Regressions ──────────────────────────────────────────
  listBenchmarkRegressions: (projectId: string, limit?: number) =>
    request<import('./types').BenchmarkRegressionWithConfig[]>(
      projectUrl(projectId, `benchmark-regressions${limit ? `?limit=${limit}` : ''}`)
    ),

  // ── Benchmark Tokens (platform admin only, NOT project-scoped) ──────
  listBenchTokens: () =>
    request<BenchTokenInfo[]>('/bench-tokens'),

  revokeBenchToken: (name: string) =>
    request<{ deleted: boolean }>(`/bench-tokens/${encodeURIComponent(name)}`, { method: 'DELETE' }),

  revokeAllBenchTokens: () =>
    request<{ deleted: number }>('/bench-tokens', { method: 'DELETE' }),

  // ── Alerting (docs/alerting.md; operator writes, member reads) ──────
  createAlertChannel: (projectId: string, body: AlertChannelCreate) =>
    request<AlertChannel>(`/v2/projects/${projectId}/alert-channels`, {
      method: 'POST',
      body: JSON.stringify(body),
    }),

  listAlertChannels: (projectId: string) =>
    request<AlertChannel[]>(`/v2/projects/${projectId}/alert-channels`),

  updateAlertChannel: (channelId: string, patch: Partial<AlertChannelCreate>) =>
    request<AlertChannel>(`/v2/alert-channels/${channelId}`, {
      method: 'PATCH',
      body: JSON.stringify(patch),
    }),

  // 409 while rules still reference the channel.
  deleteAlertChannel: (channelId: string) =>
    request<void>(`/v2/alert-channels/${channelId}`, { method: 'DELETE' }),

  testAlertChannel: (channelId: string) =>
    request<{ delivery_status: string }>(`/v2/alert-channels/${channelId}/test`, { method: 'POST' }),

  createAlertRule: (projectId: string, body: AlertRuleCreate) =>
    request<AlertRule>(`/v2/projects/${projectId}/alert-rules`, {
      method: 'POST',
      body: JSON.stringify(body),
    }),

  listAlertRules: (projectId: string) =>
    request<AlertRule[]>(`/v2/projects/${projectId}/alert-rules`),

  updateAlertRule: (ruleId: string, patch: Partial<AlertRuleCreate>) =>
    request<AlertRule>(`/v2/alert-rules/${ruleId}`, {
      method: 'PATCH',
      body: JSON.stringify(patch),
    }),

  deleteAlertRule: (ruleId: string) =>
    request<void>(`/v2/alert-rules/${ruleId}`, { method: 'DELETE' }),

  // Newest first; limit ≤ 200.
  listAlertEvents: (projectId: string, params?: { rule_id?: string; limit?: number; offset?: number }) => {
    const search = new URLSearchParams();
    if (params?.rule_id) search.set('rule_id', params.rule_id);
    if (params?.limit) search.set('limit', String(params.limit));
    if (params?.offset) search.set('offset', String(params.offset));
    const qs = search.toString();
    return request<AlertEvent[]>(`/v2/projects/${projectId}/alert-events${qs ? `?${qs}` : ''}`);
  },

  // ── LagHound SDK endpoints (Wave 2/3) ───────────────────────────────
  // Operator-write / member-read. Token is write-only: reads mask it as
  // '********'. Delete of a missing/foreign id is a flat 404 (not 403).
  listSdkEndpoints: (projectId: string) =>
    request<import('./types').SdkEndpoint[]>(projectUrl(projectId, 'sdk-endpoints')),

  getSdkEndpoint: (projectId: string, id: string) =>
    request<import('./types').SdkEndpoint>(projectUrl(projectId, `sdk-endpoints/${id}`)),

  createSdkEndpoint: (projectId: string, body: import('./types').SdkEndpointCreate) =>
    request<import('./types').SdkEndpoint>(projectUrl(projectId, 'sdk-endpoints'), {
      method: 'POST',
      body: JSON.stringify(body),
    }),

  deleteSdkEndpoint: (projectId: string, id: string) =>
    request<void>(projectUrl(projectId, `sdk-endpoints/${id}`), { method: 'DELETE' }),

  // ── Application Network Performance report (app-network) ─────────────
  // member-read. Optional config_id narrows to one SDK endpoint.
  getAppNetworkReport: (projectId: string, configId?: string) => {
    const qs = configId ? `?config_id=${encodeURIComponent(configId)}` : '';
    return request<import('./types').AppNetworkReport>(
      projectUrl(projectId, `reports/app-network${qs}`),
    );
  },
};
