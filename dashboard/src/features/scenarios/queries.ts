import { useQuery } from '@tanstack/react-query';
import { scenariosApi } from './api';

export const scenarioKeys = {
  all: ['scenarios'] as const,
  readiness: (projectId: string) => [...scenarioKeys.all, 'readiness', projectId] as const,
};

export function useScenarioReadinessQuery(projectId: string) {
  return useQuery({
    queryKey: scenarioKeys.readiness(projectId),
    queryFn: ({ signal }) => scenariosApi.loadReadiness(projectId, signal),
    enabled: !!projectId,
    refetchInterval: 15_000,
  });
}
