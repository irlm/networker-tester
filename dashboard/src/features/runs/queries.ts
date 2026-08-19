import { keepPreviousData, queryOptions, useMutation, useQueries, useQuery, useQueryClient } from '@tanstack/react-query';
import { runsApi, type RunListParams } from './api';

interface PollingOptions {
  polling?: boolean;
  intervalMs?: number;
  activeIntervalMs?: number;
  /** Gate the query off entirely (e.g. a run with no comparison group). */
  enabled?: boolean;
}

export const runKeys = {
  all: ['runs'] as const,
  lists: () => [...runKeys.all, 'list'] as const,
  list: (projectId: string, params: RunListParams) => [...runKeys.lists(), projectId, params] as const,
  detail: (runId: string) => [...runKeys.all, 'detail', runId] as const,
  attempts: (runId: string) => [...runKeys.detail(runId), 'attempts'] as const,
  artifact: (runId: string) => [...runKeys.detail(runId), 'artifact'] as const,
  infra: (runId: string) => [...runKeys.detail(runId), 'infra'] as const,
  comparison: (runIds: string[]) => [...runKeys.all, 'comparison', ...runIds] as const,
  configs: (projectId: string) => ['test-configs', projectId] as const,
  config: (configId: string) => ['test-configs', 'detail', configId] as const,
  schedules: (projectId: string) => ['test-schedules', projectId] as const,
  comparisonGroup: (groupId: string) => ['comparison-groups', 'detail', groupId] as const,
};

export function useTestConfigsQuery(projectId: string, options: PollingOptions = {}) {
  return useQuery({
    queryKey: runKeys.configs(projectId),
    queryFn: ({ signal }) => runsApi.listConfigs(projectId, signal),
    enabled: !!projectId,
    refetchInterval: options.polling === false ? false : options.intervalMs,
  });
}

export function testConfigQueryOptions(configId: string) {
  return queryOptions({
    queryKey: runKeys.config(configId),
    queryFn: ({ signal }) => runsApi.getConfig(configId, signal),
    staleTime: 60_000,
  });
}

export function useTestConfigDetailsQueries(configIds: string[]) {
  return useQueries({
    queries: configIds.map((configId) => ({
      ...testConfigQueryOptions(configId),
      enabled: !!configId,
    })),
  });
}

export function useTestRunsQuery(
  projectId: string,
  params: RunListParams,
  options: PollingOptions = {},
) {
  return useQuery({
    queryKey: runKeys.list(projectId, params),
    queryFn: ({ signal }) => runsApi.list(projectId, params, signal),
    enabled: !!projectId && (options.enabled ?? true),
    refetchInterval: options.polling === false
      ? false
      : options.activeIntervalMs
        ? (query) => {
            const rows = query.state.data as Awaited<ReturnType<typeof runsApi.list>> | undefined;
            const hasActiveRun = rows?.some((run) =>
              run.status === 'queued' || run.status === 'provisioning' || run.status === 'running');
            return hasActiveRun ? options.activeIntervalMs : (options.intervalMs ?? 15_000);
          }
        : (options.intervalMs ?? 15_000),
    placeholderData: keepPreviousData,
  });
}

export function useTestRunQuery(runId: string, polling = true) {
  return useQuery({
    queryKey: runKeys.detail(runId),
    queryFn: ({ signal }) => runsApi.get(runId, signal),
    enabled: !!runId,
    refetchInterval: polling
      ? (query) => {
          const run = query.state.data as Awaited<ReturnType<typeof runsApi.get>> | undefined;
          const isActive = !run
            || run.status === 'queued'
            || run.status === 'provisioning'
            || run.status === 'running';
          return isActive ? 15_000 : false;
        }
      : false,
  });
}

export function useRunAttemptsQuery(runId: string, polling = true) {
  return useQuery({
    queryKey: runKeys.attempts(runId),
    queryFn: ({ signal }) => runsApi.getAttempts(runId, signal),
    enabled: !!runId,
    refetchInterval: polling ? 15_000 : false,
  });
}

export function useRunArtifactQuery(runId: string, artifactId?: string | null) {
  return useQuery({
    queryKey: runKeys.artifact(runId),
    queryFn: ({ signal }) => runsApi.getArtifact(runId, signal),
    enabled: !!runId && !!artifactId,
    staleTime: Infinity,
  });
}

/** One attempts query per run — the comparison page's per-cell stat source.
 *  Shares runKeys.attempts(runId) with the run-detail page cache. */
export function useRunsAttemptsQueries(runIds: string[]) {
  return useQueries({
    queries: runIds.map((runId) => ({
      queryKey: runKeys.attempts(runId),
      queryFn: ({ signal }: { signal?: AbortSignal }) => runsApi.getAttempts(runId, signal),
      enabled: !!runId,
      staleTime: 60_000,
    })),
  });
}

/** One artifact query per run with an artifact_id (post-#796 per-case depth). */
export function useRunsArtifactsQueries(runs: { id: string; artifact_id: string | null }[]) {
  return useQueries({
    queries: runs.map((run) => ({
      queryKey: runKeys.artifact(run.id),
      queryFn: ({ signal }: { signal?: AbortSignal }) => runsApi.getArtifact(run.id, signal),
      enabled: !!run.id && !!run.artifact_id,
      staleTime: Infinity,
    })),
  });
}

export function useComparisonGroupQuery(groupId: string) {
  return useQuery({
    queryKey: runKeys.comparisonGroup(groupId),
    queryFn: ({ signal }) => runsApi.getComparisonGroup(groupId, signal),
    enabled: !!groupId,
    staleTime: 60_000,
    // The group row may 404 for older runs whose group was deleted (ON DELETE
    // SET NULL keeps the runs) — the page degrades to name parsing.
    retry: false,
  });
}

/**
 * One group-detail query per comparison group visible on the runs list —
 * authoritative cell counts + name for the group rows (#803). Shares
 * runKeys.comparisonGroup with the single-group hook. retry: false because a
 * 404 is a real answer (deleted group, SET NULL cells) the row falls back on.
 */
export function useComparisonGroupsQueries(groupIds: string[]) {
  return useQueries({
    queries: groupIds.map((groupId) => ({
      queryKey: runKeys.comparisonGroup(groupId),
      queryFn: ({ signal }: { signal?: AbortSignal }) => runsApi.getComparisonGroup(groupId, signal),
      enabled: !!groupId,
      staleTime: 60_000,
      retry: false,
    })),
  });
}

export function useDeleteComparisonGroupMutation() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (groupId: string) => runsApi.deleteComparisonGroup(groupId),
    // The cells' runs survive but lose their group linkage — refresh both the
    // run lists (group chips/rows) and the cached group detail.
    onSuccess: (_void, groupId) => Promise.all([
      client.invalidateQueries({ queryKey: runKeys.lists() }),
      client.removeQueries({ queryKey: runKeys.comparisonGroup(groupId) }),
    ]),
  });
}

export function useRunInfraQuery(runId: string) {
  return useQuery({
    queryKey: runKeys.infra(runId),
    queryFn: ({ signal }) => runsApi.getInfra(runId, signal),
    enabled: !!runId,
    staleTime: Infinity,
    retry: false,
  });
}

export function useRunComparisonQuery(runIds: string[]) {
  return useQuery({
    queryKey: runKeys.comparison(runIds),
    queryFn: ({ signal }) => runsApi.compare(runIds, signal),
    enabled: runIds.length >= 2,
  });
}

export function useSchedulesQuery(projectId: string, polling = true) {
  return useQuery({
    queryKey: runKeys.schedules(projectId),
    queryFn: ({ signal }) => runsApi.listSchedules(projectId, signal),
    enabled: !!projectId,
    refetchInterval: polling ? 10_000 : false,
  });
}

export function useUpdateScheduleMutation(projectId: string) {
  const client = useQueryClient();
  return useMutation({
    mutationFn: ({ scheduleId, enabled }: { scheduleId: string; enabled: boolean }) =>
      runsApi.updateSchedule(scheduleId, { enabled }),
    onSuccess: () => client.invalidateQueries({ queryKey: runKeys.schedules(projectId) }),
  });
}

export function useTriggerScheduleMutation(projectId: string) {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (scheduleId: string) => runsApi.triggerSchedule(scheduleId),
    onSuccess: () => Promise.all([
      client.invalidateQueries({ queryKey: runKeys.schedules(projectId) }),
      client.invalidateQueries({ queryKey: runKeys.lists() }),
    ]),
  });
}

export function useDeleteScheduleMutation(projectId: string) {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (scheduleId: string) => runsApi.deleteSchedule(scheduleId),
    onSuccess: () => client.invalidateQueries({ queryKey: runKeys.schedules(projectId) }),
  });
}

export function useCancelRunMutation(runId: string) {
  const client = useQueryClient();
  return useMutation({
    mutationFn: () => runsApi.cancel(runId),
    onSuccess: () => client.invalidateQueries({ queryKey: runKeys.detail(runId) }),
  });
}

/** Single-config detail (baseline pin state on the run page). */
export function useTestConfigQuery(configId: string, enabled = true) {
  return useQuery({
    ...testConfigQueryOptions(configId),
    enabled: !!configId && enabled,
  });
}

/**
 * Pin/unpin this run as its config's regression baseline (#810). Refreshes
 * the cached config detail so the page's pinned state flips immediately.
 */
export function usePinBaselineMutation(runId: string, configId: string | undefined) {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (pin: boolean) => (pin ? runsApi.pinBaseline(runId) : runsApi.unpinBaseline(runId)),
    onSuccess: () => (configId
      ? client.invalidateQueries({ queryKey: runKeys.config(configId) })
      : undefined),
  });
}
