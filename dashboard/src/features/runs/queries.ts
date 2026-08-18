import { keepPreviousData, queryOptions, useMutation, useQueries, useQuery, useQueryClient } from '@tanstack/react-query';
import { runsApi, type RunListParams } from './api';

interface PollingOptions {
  polling?: boolean;
  intervalMs?: number;
  activeIntervalMs?: number;
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
    enabled: !!projectId,
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
    refetchInterval: polling ? 15_000 : false,
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
