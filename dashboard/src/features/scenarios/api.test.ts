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
  it('uses the same tester inventory as the launch pages', async () => {
    fetchMock.mockResolvedValueOnce(jsonResponse([{ tester_id: 't-1' }]));
    await expect(scenariosApi.listRunners('p-1')).resolves.toEqual([{ tester_id: 't-1' }]);
    expect(fetchMock).toHaveBeenCalledWith('/api/projects/p-1/testers', expect.anything());
  });

  it('keeps successful readiness resources when one API fails', async () => {
    vi.spyOn(scenariosApi, 'listRunners').mockResolvedValue([]);
    vi.spyOn(scenariosApi, 'listDeployments').mockResolvedValue([]);
    vi.spyOn(scenariosApi, 'listCloudAccounts').mockRejectedValue(new Error('Cloud API unavailable'));

    const result = await scenariosApi.loadReadiness('p-1');

    expect(result.runners).toEqual({ data: [], error: null });
    expect(result.deployments).toEqual({ data: [], error: null });
    expect(result.cloudAccounts).toEqual({ data: null, error: 'Cloud API unavailable' });
  });
});
