/**
 * Persistent tester REST client.
 *
 * Wraps the routes mounted by `crates/networker-dashboard/src/api/testers.rs`
 * (Tasks 14-17) into typed async functions. All endpoints are nested under
 * `/api/projects/{projectId}/testers`.
 *
 * Note: the backend serves snake_case JSON. We preserve that shape in TS
 * types to avoid a transform layer — components can read fields directly.
 */

import { request } from './client';

export type PowerState =
  | 'provisioning'
  | 'starting'
  | 'running'
  | 'stopping'
  | 'stopped'
  | 'upgrading'
  | 'error';

export type Allocation = 'idle' | 'locked' | 'upgrading';

export type TesterRow = {
  tester_id: string;
  project_id: string;
  name: string;
  cloud: string;
  region: string;
  vm_size: string;
  requested_os?: string | null;
  requested_variant?: string | null;
  vm_name: string | null;
  public_ip: string | null;
  ssh_user: string;
  power_state: PowerState;
  allocation: Allocation;
  status_message: string | null;
  locked_by_config_id: string | null;
  installer_version: string | null;
  last_installed_at: string | null;
  auto_shutdown_enabled: boolean;
  auto_shutdown_local_hour: number;
  next_shutdown_at: string | null;
  shutdown_deferral_count: number;
  auto_probe_enabled: boolean;
  last_used_at: string | null;
  avg_benchmark_duration_seconds: number | null;
  benchmark_run_count: number;
  created_by: string;
  created_at: string;
  updated_at: string;
};

export type TesterQueueRunning = {
  config_id: string;
  name: string;
  started_at: string | null;
};

export type TesterQueueQueued = {
  config_id: string;
  name: string;
  queued_at: string | null;
  position: number;
  eta: string | null;
};

export type TesterQueue = {
  tester_id: string;
  running: TesterQueueRunning | null;
  queued: TesterQueueQueued[];
};

export type CostEstimate = {
  vm_size: string;
  hourly_usd: number;
  monthly_always_on_usd: number;
  monthly_with_schedule_usd: number;
  auto_shutdown_enabled: boolean;
};

export type CreateTesterBody = {
  name: string;
  cloud: string;
  region: string;
  vm_size?: string;
  auto_shutdown_local_hour?: number;
  auto_probe_enabled?: boolean;
  cloud_account_id?: string;
  requested_os?: string;
  requested_variant?: string;
};

export type ScheduleBody = {
  auto_shutdown_enabled?: boolean;
  auto_shutdown_local_hour?: number;
};

export type PostponeBody =
  | { until: string }
  | { add_hours: number }
  | { skip_tonight: boolean };

export type ForceStopBody = {
  confirm: boolean;
  reason?: string;
};

export type RefreshLatestVersionResponse = {
  latest_version: string;
};

function base(projectId: string, suffix = ''): string {
  return `/projects/${projectId}/testers${suffix}`;
}

export const testersApi = {
  listTesters: (projectId: string) =>
    request<TesterRow[]>(base(projectId)),

  getTester: (projectId: string, testerId: string) =>
    request<TesterRow>(base(projectId, `/${testerId}`)),

  getRegions: (projectId: string) =>
    request<{ regions: string[] }>(base(projectId, '/regions')).then(
      (r) => r.regions,
    ),

  getQueue: (projectId: string, testerId: string) =>
    request<TesterQueue>(base(projectId, `/${testerId}/queue`)),

  getCostEstimate: (projectId: string, testerId: string) =>
    request<CostEstimate>(base(projectId, `/${testerId}/cost_estimate`)),

  createTester: (projectId: string, body: CreateTesterBody) =>
    request<TesterRow>(base(projectId), {
      method: 'POST',
      body: JSON.stringify(body),
    }),

  startTester: (projectId: string, testerId: string) =>
    request<TesterRow>(base(projectId, `/${testerId}/start`), {
      method: 'POST',
    }),

  stopTester: (projectId: string, testerId: string) =>
    request<TesterRow>(base(projectId, `/${testerId}/stop`), {
      method: 'POST',
    }),

  upgradeTester: (
    projectId: string,
    testerId: string,
    opts: { confirm: boolean },
  ) =>
    request<TesterRow>(base(projectId, `/${testerId}/upgrade`), {
      method: 'POST',
      body: JSON.stringify(opts),
    }),

  deleteTester: (projectId: string, testerId: string) =>
    request<{ deleted: boolean }>(base(projectId, `/${testerId}`), {
      method: 'DELETE',
    }),

  updateSchedule: (
    projectId: string,
    testerId: string,
    body: ScheduleBody,
  ) =>
    request<TesterRow>(base(projectId, `/${testerId}/schedule`), {
      method: 'PATCH',
      body: JSON.stringify(body),
    }),

  postpone: (projectId: string, testerId: string, body: PostponeBody) =>
    request<TesterRow>(base(projectId, `/${testerId}/postpone`), {
      method: 'POST',
      body: JSON.stringify(body),
    }),

  probe: (projectId: string, testerId: string) =>
    request<TesterRow>(base(projectId, `/${testerId}/probe`), {
      method: 'POST',
    }),

  forceStop: (projectId: string, testerId: string, body: ForceStopBody) =>
    request<TesterRow>(base(projectId, `/${testerId}/force-stop`), {
      method: 'POST',
      body: JSON.stringify(body),
    }),

  refreshLatestVersion: (projectId: string) =>
    request<RefreshLatestVersionResponse>(
      base(projectId, '/refresh-latest-version'),
      { method: 'POST' },
    ),
};
