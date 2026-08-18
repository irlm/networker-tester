import { keepPreviousData, useQuery } from '@tanstack/react-query';
import { perfLogsApi, type PerfLogListParams } from './api';

export const perfLogKeys = {
  all: ['perf-logs'] as const,
  lists: () => [...perfLogKeys.all, 'list'] as const,
  list: (params: PerfLogListParams) => [...perfLogKeys.lists(), params] as const,
  stats: (windowMs?: number) => [...perfLogKeys.all, 'stats', windowMs ?? 'all'] as const,
};

export function usePerfLogsQuery(params: PerfLogListParams, polling = true) {
  return useQuery({
    queryKey: perfLogKeys.list(params),
    queryFn: ({ signal }) => perfLogsApi.list(params, signal),
    placeholderData: keepPreviousData,
    refetchInterval: polling ? 15_000 : false,
  });
}

export function usePerfLogStatsQuery(windowMs?: number) {
  return useQuery({
    queryKey: perfLogKeys.stats(windowMs),
    queryFn: ({ signal }) => perfLogsApi.stats(windowMs, signal),
    placeholderData: keepPreviousData,
    refetchInterval: 15_000,
  });
}
