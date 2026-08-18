import type { PropsWithChildren } from 'react';
import { act, renderHook, waitFor } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { TestRun } from '../../api/types';
import { runsApi, type RunListParams } from './api';
import {
  runKeys,
  testConfigQueryOptions,
  useCancelRunMutation,
  useTestConfigDetailsQueries,
  useTestConfigsQuery,
  useTestRunQuery,
  useTestRunsQuery,
} from './queries';

function createClient() {
  return new QueryClient({
    defaultOptions: {
      queries: { retry: false, gcTime: Infinity },
      mutations: { retry: false },
    },
  });
}

function wrapperFor(client: QueryClient) {
  return function QueryWrapper({ children }: PropsWithChildren) {
    return <QueryClientProvider client={client}>{children}</QueryClientProvider>;
  };
}

function cachedRefetchInterval(client: QueryClient, queryKey: readonly unknown[]): unknown {
  const query = client.getQueryCache().find({ queryKey });
  return (query?.options as { refetchInterval?: unknown } | undefined)?.refetchInterval;
}

const completedRun: TestRun = {
  id: 'run-1',
  test_config_id: 'config-1',
  project_id: 'project-1',
  status: 'completed',
  started_at: '2026-08-18T12:00:00Z',
  finished_at: '2026-08-18T12:01:00Z',
  success_count: 10,
  failure_count: 0,
  error_message: null,
  artifact_id: null,
  tester_id: 'tester-1',
  worker_id: null,
  last_heartbeat: null,
  created_at: '2026-08-18T11:59:00Z',
};

describe('run query architecture', () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('builds hierarchical keys that isolate lists, details, and detail resources', () => {
    const params: RunListParams = { status: 'completed', limit: 25 };

    expect(runKeys.all).toEqual(['runs']);
    expect(runKeys.lists()).toEqual(['runs', 'list']);
    expect(runKeys.list('project-1', params)).toEqual(['runs', 'list', 'project-1', params]);
    expect(runKeys.detail('run-1')).toEqual(['runs', 'detail', 'run-1']);
    expect(runKeys.attempts('run-1')).toEqual(['runs', 'detail', 'run-1', 'attempts']);
    expect(runKeys.artifact('run-1')).toEqual(['runs', 'detail', 'run-1', 'artifact']);
    expect(runKeys.infra('run-1')).toEqual(['runs', 'detail', 'run-1', 'infra']);
    expect(runKeys.config('config-1')).toEqual(['test-configs', 'detail', 'config-1']);
    expect(testConfigQueryOptions('config-1').queryKey).toEqual(runKeys.config('config-1'));
  });

  it('does not issue project-scoped requests until a project exists', () => {
    const listSpy = vi.spyOn(runsApi, 'list').mockResolvedValue([]);
    const configsSpy = vi.spyOn(runsApi, 'listConfigs').mockResolvedValue([]);
    const client = createClient();

    const { result } = renderHook(() => ({
      runs: useTestRunsQuery('', {}),
      configs: useTestConfigsQuery(''),
    }), { wrapper: wrapperFor(client) });

    expect(result.current.runs.fetchStatus).toBe('idle');
    expect(result.current.configs.fetchStatus).toBe('idle');
    expect(listSpy).not.toHaveBeenCalled();
    expect(configsSpy).not.toHaveBeenCalled();
  });

  it('forwards cancellation and stores the requested polling interval', async () => {
    const listSpy = vi.spyOn(runsApi, 'list').mockResolvedValue([]);
    const client = createClient();
    const params = { status: 'running' };
    const { result } = renderHook(
      () => useTestRunsQuery('project-1', params, { intervalMs: 2_500 }),
      { wrapper: wrapperFor(client) },
    );

    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    expect(listSpy).toHaveBeenCalledWith('project-1', params, expect.any(AbortSignal));
    expect(cachedRefetchInterval(client, runKeys.list('project-1', params))).toBe(2_500);
  });

  it('can explicitly disable polling', async () => {
    vi.spyOn(runsApi, 'list').mockResolvedValue([]);
    const client = createClient();
    const params = { status: 'completed' };
    const { result } = renderHook(
      () => useTestRunsQuery('project-1', params, { polling: false }),
      { wrapper: wrapperFor(client) },
    );

    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    expect(cachedRefetchInterval(client, runKeys.list('project-1', params))).toBe(false);
  });

  it('polls active run lists faster without page-owned timers', async () => {
    vi.spyOn(runsApi, 'list').mockResolvedValue([{ ...completedRun, status: 'running' }]);
    const client = createClient();
    const params = { endpoint_kind: 'network', limit: 200 };
    const { result } = renderHook(
      () => useTestRunsQuery('project-1', params, { intervalMs: 15_000, activeIntervalMs: 5_000 }),
      { wrapper: wrapperFor(client) },
    );

    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    const query = client.getQueryCache().find({ queryKey: runKeys.list('project-1', params) });
    const interval = cachedRefetchInterval(client, runKeys.list('project-1', params));
    expect(typeof interval).toBe('function');
    expect((interval as (value: typeof query) => number)(query)).toBe(5_000);
  });

  it('stops polling run metadata after the run reaches a terminal state', async () => {
    vi.spyOn(runsApi, 'get').mockResolvedValue(completedRun);
    const client = createClient();
    const { result } = renderHook(
      () => useTestRunQuery('run-1'),
      { wrapper: wrapperFor(client) },
    );

    await waitFor(() => expect(result.current.data).toEqual(completedRun));
    const query = client.getQueryCache().find({ queryKey: runKeys.detail('run-1') });
    const interval = cachedRefetchInterval(client, runKeys.detail('run-1'));
    expect(typeof interval).toBe('function');
    expect((interval as (value: typeof query) => number | false)(query)).toBe(false);
  });

  it('shares full config detail requests through stable keys', async () => {
    const config = {
      id: 'config-1',
      project_id: 'project-1',
      name: 'Network config',
      description: null,
      endpoint: { kind: 'network' as const, host: 'https://example.com' },
      workload: { modes: ['http2'], runs: 1, concurrency: 1, timeout_ms: 5_000, payload_sizes: [], capture_mode: 'headers-only' as const },
      methodology: null,
      baseline_run_id: null,
      max_duration_secs: 60,
      created_by: null,
      created_at: '2026-08-18T12:00:00Z',
      updated_at: '2026-08-18T12:00:00Z',
    };
    const getConfigSpy = vi.spyOn(runsApi, 'getConfig').mockResolvedValue(config);
    const client = createClient();
    const { result } = renderHook(
      () => useTestConfigDetailsQueries(['config-1']),
      { wrapper: wrapperFor(client) },
    );

    await waitFor(() => expect(result.current[0].data).toEqual(config));
    expect(getConfigSpy).toHaveBeenCalledWith('config-1', expect.any(AbortSignal));
    expect(client.getQueryData(runKeys.config('config-1'))).toEqual(config);
  });

  it('keeps prior rows visible while a changed filter fetches', async () => {
    let resolveSecond!: (runs: TestRun[]) => void;
    const listSpy = vi.spyOn(runsApi, 'list').mockImplementation((_projectId, params) => {
      if (params?.status === 'completed') return Promise.resolve([completedRun]);
      return new Promise<TestRun[]>(resolve => { resolveSecond = resolve; });
    });
    const client = createClient();
    const { result, rerender } = renderHook(
      ({ status }: { status: string }) => useTestRunsQuery('project-1', { status }, { polling: false }),
      { initialProps: { status: 'completed' }, wrapper: wrapperFor(client) },
    );

    await waitFor(() => expect(result.current.data).toEqual([completedRun]));
    rerender({ status: 'failed' });
    await waitFor(() => expect(listSpy).toHaveBeenCalledTimes(2));

    expect(result.current.data).toEqual([completedRun]);
    expect(result.current.isPlaceholderData).toBe(true);

    act(() => resolveSecond([]));
    await waitFor(() => expect(result.current.data).toEqual([]));
    expect(result.current.isPlaceholderData).toBe(false);
  });

  it('aborts an in-flight request when its final observer unmounts', async () => {
    let requestSignal: AbortSignal | undefined;
    vi.spyOn(runsApi, 'list').mockImplementation((_projectId, _params, signal) => {
      requestSignal = signal;
      return new Promise<TestRun[]>(() => {});
    });
    const client = createClient();
    const { unmount } = renderHook(
      () => useTestRunsQuery('project-1', { status: 'running' }, { polling: false }),
      { wrapper: wrapperFor(client) },
    );

    await waitFor(() => expect(requestSignal).toBeDefined());
    expect(requestSignal?.aborted).toBe(false);
    unmount();
    await waitFor(() => expect(requestSignal?.aborted).toBe(true));
  });

  it('invalidates the run detail after a successful cancellation', async () => {
    const cancelSpy = vi.spyOn(runsApi, 'cancel').mockResolvedValue(undefined);
    const client = createClient();
    const invalidateSpy = vi.spyOn(client, 'invalidateQueries').mockResolvedValue(undefined);
    const { result } = renderHook(
      () => useCancelRunMutation('run-1'),
      { wrapper: wrapperFor(client) },
    );

    await act(() => result.current.mutateAsync());

    expect(cancelSpy).toHaveBeenCalledWith('run-1');
    expect(invalidateSpy).toHaveBeenCalledWith({ queryKey: runKeys.detail('run-1') });
  });
});
