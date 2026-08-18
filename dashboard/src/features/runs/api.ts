import { request } from '../../api/http';
import type {
  BenchmarkArtifact,
  ComparisonGroup,
  ComparisonGroupCreate,
  ComparisonReport,
  LiveAttempt,
  RunInfra,
  TestConfig,
  TestConfigCreate,
  TestConfigListItem,
  TestRun,
  TestSchedule,
} from '../../api/types';

export interface RunListParams {
  status?: string;
  endpoint_kind?: string;
  has_artifact?: boolean;
  comparison_group_id?: string;
  limit?: number;
  before?: string;
}

function withSignal(signal?: AbortSignal): RequestInit | undefined {
  return signal ? { signal } : undefined;
}

/** Feature-owned TestConfig/TestRun/TestSchedule transport. */
export const runsApi = {
  createConfig: (projectId: string, config: TestConfigCreate) =>
    request<TestConfig>(`/v2/projects/${projectId}/test-configs`, {
      method: 'POST',
      body: JSON.stringify(config),
    }),

  listConfigs: (projectId: string, signal?: AbortSignal) =>
    request<TestConfigListItem[]>(`/v2/projects/${projectId}/test-configs`, withSignal(signal)),

  getConfig: (configId: string, signal?: AbortSignal) =>
    request<TestConfig>(`/v2/test-configs/${configId}`, withSignal(signal)),

  updateConfig: (configId: string, patch: Partial<TestConfigCreate>) =>
    request<TestConfig>(`/v2/test-configs/${configId}`, {
      method: 'PATCH',
      body: JSON.stringify(patch),
    }),

  deleteConfig: (configId: string) =>
    request<void>(`/v2/test-configs/${configId}`, { method: 'DELETE' }),

  launchConfig: (configId: string, testerId?: string) =>
    request<TestRun>(`/v2/test-configs/${configId}/launch`, {
      method: 'POST',
      body: JSON.stringify({ tester_id: testerId }),
    }),

  list: (projectId: string, params: RunListParams = {}, signal?: AbortSignal) => {
    const search = new URLSearchParams();
    if (params.status) search.set('status', params.status);
    if (params.endpoint_kind) search.set('endpoint_kind', params.endpoint_kind);
    if (params.has_artifact !== undefined) search.set('has_artifact', String(params.has_artifact));
    if (params.comparison_group_id) search.set('comparison_group_id', params.comparison_group_id);
    if (params.limit) search.set('limit', String(params.limit));
    if (params.before) search.set('before', params.before);
    const qs = search.toString();
    return request<TestRun[]>(`/v2/projects/${projectId}/test-runs${qs ? `?${qs}` : ''}`, withSignal(signal));
  },

  get: (runId: string, signal?: AbortSignal) =>
    request<TestRun>(`/v2/test-runs/${runId}`, withSignal(signal)),

  getArtifact: (runId: string, signal?: AbortSignal) =>
    request<BenchmarkArtifact>(`/v2/test-runs/${runId}/artifact`, withSignal(signal)),

  getInfra: (runId: string, signal?: AbortSignal) =>
    request<RunInfra>(`/v2/test-runs/${runId}/infra`, withSignal(signal)),

  getAttempts: (runId: string, signal?: AbortSignal) =>
    request<{ attempts: LiveAttempt[] } | LiveAttempt[]>(
      `/v2/test-runs/${runId}/attempts`,
      withSignal(signal),
    ).then((response) => (Array.isArray(response) ? response : response?.attempts ?? [])),

  cancel: (runId: string) =>
    request<void>(`/v2/test-runs/${runId}/cancel`, { method: 'POST' }),

  compare: (runIds: string[], signal?: AbortSignal) =>
    request<ComparisonReport>('/v2/test-runs/compare', {
      method: 'POST',
      body: JSON.stringify({ run_ids: runIds }),
      signal,
    }),

  createSchedule: (projectId: string, params: {
    test_config_id: string;
    cron_expr: string;
    timezone?: string;
    enabled?: boolean;
  }) => request<TestSchedule>(`/v2/projects/${projectId}/schedules`, {
    method: 'POST',
    body: JSON.stringify(params),
  }),

  listSchedules: (projectId: string, signal?: AbortSignal) =>
    request<TestSchedule[]>(`/v2/projects/${projectId}/schedules`, withSignal(signal)),

  updateSchedule: (scheduleId: string, patch: Partial<{
    cron_expr: string;
    timezone: string;
    enabled: boolean;
  }>) => request<TestSchedule>(`/v2/schedules/${scheduleId}`, {
    method: 'PATCH',
    body: JSON.stringify(patch),
  }),

  deleteSchedule: (scheduleId: string) =>
    request<void>(`/v2/schedules/${scheduleId}`, { method: 'DELETE' }),

  triggerSchedule: (scheduleId: string) =>
    request<TestRun>(`/v2/schedules/${scheduleId}/trigger`, { method: 'POST' }),

  createComparisonGroup: (projectId: string, body: ComparisonGroupCreate) =>
    request<ComparisonGroup>(`/v2/projects/${projectId}/comparison-groups`, {
      method: 'POST',
      body: JSON.stringify(body),
    }),

  listComparisonGroups: (projectId: string, signal?: AbortSignal) =>
    request<ComparisonGroup[]>(`/v2/projects/${projectId}/comparison-groups`, withSignal(signal)),

  getComparisonGroup: (groupId: string, signal?: AbortSignal) =>
    request<ComparisonGroup>(`/v2/comparison-groups/${groupId}`, withSignal(signal)),

  launchComparisonGroup: (groupId: string) =>
    request<ComparisonGroup>(`/v2/comparison-groups/${groupId}/launch`, { method: 'POST' }),
};
