import { request, errorMessage } from '../../api/http';
import type { Agent, CloudAccountSummary, Deployment } from '../../api/types';

export interface ReadinessResource<T> {
  data: T[] | null;
  error: string | null;
}

export interface ScenarioReadinessResponse {
  agents: ReadinessResource<Agent>;
  deployments: ReadinessResource<Deployment>;
  cloudAccounts: ReadinessResource<CloudAccountSummary>;
}

function withSignal(signal?: AbortSignal): RequestInit | undefined {
  return signal ? { signal } : undefined;
}

function settledResource<T>(result: PromiseSettledResult<T[]>): ReadinessResource<T> {
  return result.status === 'fulfilled'
    ? { data: result.value, error: null }
    : { data: null, error: errorMessage(result.reason) };
}

export const scenariosApi = {
  listAgents: (projectId: string, signal?: AbortSignal) =>
    request<Agent[] | { agents: Agent[] }>(
      `/projects/${projectId}/agents`,
      withSignal(signal),
    ).then((response) => (Array.isArray(response) ? response : response?.agents ?? [])),

  listDeployments: (projectId: string, signal?: AbortSignal) =>
    request<Deployment[]>(
      `/projects/${projectId}/deployments?limit=50`,
      withSignal(signal),
    ),

  listCloudAccounts: (projectId: string, signal?: AbortSignal) =>
    request<CloudAccountSummary[]>(
      `/projects/${projectId}/cloud-accounts`,
      withSignal(signal),
    ),

  async loadReadiness(projectId: string, signal?: AbortSignal): Promise<ScenarioReadinessResponse> {
    const [agents, deployments, cloudAccounts] = await Promise.allSettled([
      scenariosApi.listAgents(projectId, signal),
      scenariosApi.listDeployments(projectId, signal),
      scenariosApi.listCloudAccounts(projectId, signal),
    ]);

    if (signal?.aborted) throw new DOMException('Aborted', 'AbortError');

    return {
      agents: settledResource(agents),
      deployments: settledResource(deployments),
      cloudAccounts: settledResource(cloudAccounts),
    };
  },
};
