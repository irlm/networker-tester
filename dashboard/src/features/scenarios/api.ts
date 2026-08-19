import { request, errorMessage } from '../../api/http';
import { testersApi, type TesterRow } from '../../api/testers';
import type { CloudAccountSummary, Deployment } from '../../api/types';

export interface ReadinessResource<T> {
  data: T[] | null;
  error: string | null;
}

export interface ScenarioReadinessResponse {
  runners: ReadinessResource<TesterRow>;
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
  listRunners: (projectId: string, signal?: AbortSignal) =>
    testersApi.listTesters(projectId, signal),

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
    const [runners, deployments, cloudAccounts] = await Promise.allSettled([
      scenariosApi.listRunners(projectId, signal),
      scenariosApi.listDeployments(projectId, signal),
      scenariosApi.listCloudAccounts(projectId, signal),
    ]);

    if (signal?.aborted) throw new DOMException('Aborted', 'AbortError');

    return {
      runners: settledResource(runners),
      deployments: settledResource(deployments),
      cloudAccounts: settledResource(cloudAccounts),
    };
  },
};
