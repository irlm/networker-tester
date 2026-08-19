import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { useApiLogStore } from '../../stores/apiLogStore';
import { scenariosApi } from './api';

function jsonResponse(body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status: 200,
    headers: { 'Content-Type': 'application/json' },
  });
}

const fetchMock = vi.fn();

beforeEach(() => {
  vi.stubGlobal('fetch', fetchMock);
  fetchMock.mockReset();
  useApiLogStore.setState({ enabled: false });
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe('scenarios API', () => {
  it('normalizes both supported runner response shapes', async () => {
    fetchMock.mockResolvedValueOnce(jsonResponse({ agents: [{ agent_id: 'a-1' }] }));
    await expect(scenariosApi.listAgents('p-1')).resolves.toEqual([{ agent_id: 'a-1' }]);

    fetchMock.mockResolvedValueOnce(jsonResponse([{ agent_id: 'a-2' }]));
    await expect(scenariosApi.listAgents('p-1')).resolves.toEqual([{ agent_id: 'a-2' }]);
  });

  it('keeps successful readiness resources when one API fails', async () => {
    vi.spyOn(scenariosApi, 'listAgents').mockResolvedValue([]);
    vi.spyOn(scenariosApi, 'listDeployments').mockResolvedValue([]);
    vi.spyOn(scenariosApi, 'listCloudAccounts').mockRejectedValue(new Error('Cloud API unavailable'));

    const result = await scenariosApi.loadReadiness('p-1');

    expect(result.agents).toEqual({ data: [], error: null });
    expect(result.deployments).toEqual({ data: [], error: null });
    expect(result.cloudAccounts).toEqual({ data: null, error: 'Cloud API unavailable' });
  });
});
