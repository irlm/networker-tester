import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter } from 'react-router';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { TestRun } from '../api/types';
import { RunsPage } from './RunsPage';

const mocks = vi.hoisted(() => ({
  useTestRunsQuery: vi.fn(),
  refetch: vi.fn(),
}));

vi.mock('../features/runs/queries', () => ({
  useTestRunsQuery: mocks.useTestRunsQuery,
}));
vi.mock('../hooks/useProject', () => ({ useProject: () => ({ projectId: 'project-1' }) }));
vi.mock('../hooks/useRenderLog', () => ({ useRenderLog: () => vi.fn() }));

const baseRun = {
  project_id: 'project-1',
  status: 'completed',
  started_at: '2026-08-18T12:00:00Z',
  finished_at: '2026-08-18T12:00:05Z',
  success_count: 2,
  failure_count: 0,
  error_message: null,
  artifact_id: null,
  tester_id: null,
  worker_id: null,
  last_heartbeat: null,
  created_at: '2026-08-18T12:00:00Z',
} satisfies Omit<TestRun, 'id' | 'test_config_id'>;

const runs: TestRun[] = [
  {
    ...baseRun,
    id: '11111111-1111-4111-8111-111111111111',
    test_config_id: 'config-network',
    config_name: 'Checkout connectivity',
    endpoint_kind: 'proxy',
    test_kind: 'network',
    modes: ['tcp', 'dns'],
  },
  {
    ...baseRun,
    id: '22222222-2222-4222-8222-222222222222',
    test_config_id: 'config-url',
    config_name: 'Diag: api.example.com (Quick)',
    endpoint_kind: 'network',
    test_kind: 'url_probe',
    modes: ['http2'],
  },
  {
    ...baseRun,
    id: '33333333-3333-4333-8333-333333333333',
    test_config_id: 'config-sdk',
    config_name: 'Payments SDK',
    endpoint_kind: 'network',
    test_kind: 'sdk_probe',
    modes: ['sdkprobe'],
  },
  {
    ...baseRun,
    id: '44444444-4444-4444-8444-444444444444',
    test_config_id: 'config-benchmark',
    config_name: 'Runtime throughput',
    endpoint_kind: 'runtime',
    test_kind: 'benchmark',
    modes: ['apibench', 'download'],
    artifact_id: 'artifact-1',
  },
];

function renderPage(entry = '/projects/project-1/runs') {
  return render(
    <MemoryRouter initialEntries={[entry]}>
      <RunsPage />
    </MemoryRouter>,
  );
}

describe('RunsPage filters', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.useTestRunsQuery.mockReturnValue({
      data: runs,
      isPending: false,
      isError: false,
      dataUpdatedAt: Date.now(),
      refetch: mocks.refetch,
    });
  });

  it('shows a dedicated tab for every run purpose', () => {
    renderPage();

    expect(screen.getByRole('button', { name: 'All' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Network tests' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'URL probes' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'SDK probes' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Benchmarks' })).toBeInTheDocument();
  });

  it('filters URL probes independently from ordinary network targets', async () => {
    const user = userEvent.setup();
    renderPage();

    await user.click(screen.getByRole('button', { name: 'URL probes' }));

    expect(screen.getByText('Diag: api.example.com (Quick)')).toBeInTheDocument();
    expect(screen.queryByText('Checkout connectivity')).not.toBeInTheDocument();
    expect(screen.queryByText('Payments SDK')).not.toBeInTheDocument();
    await waitFor(() => {
      expect(mocks.useTestRunsQuery.mock.calls.at(-1)?.[1]).toMatchObject({ test_kind: 'url_probe' });
    });
  });

  it('combines target and mode-family filters without changing purpose semantics', async () => {
    const user = userEvent.setup();
    renderPage();

    await user.selectOptions(screen.getByLabelText('Filter by target type'), 'runtime');
    await user.selectOptions(screen.getByLabelText('Filter by mode family'), 'thru');

    expect(screen.getByText('Runtime throughput')).toBeInTheDocument();
    expect(screen.queryByText('Checkout connectivity')).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Benchmarks' })).toBeInTheDocument();
  });

  it('passes URL-backed name, target, and time filters to the list API', async () => {
    renderPage('/projects/project-1/runs?q=Diag&time=24h&endpoint_kind=network');

    await waitFor(() => {
      const params = mocks.useTestRunsQuery.mock.calls.at(-1)?.[1];
      expect(params).toMatchObject({ q: 'Diag', endpoint_kind: 'network', limit: 200 });
      expect(Date.parse(params.since)).not.toBeNaN();
    });
    expect(screen.getByDisplayValue('Diag')).toBeInTheDocument();
    expect(screen.getByText('Diag: api.example.com (Quick)')).toBeInTheDocument();
  });

  it('keeps all time as the default so existing run history stays visible', () => {
    renderPage();

    expect(screen.getByLabelText('Filter by time range')).toHaveValue('all');
    const params = mocks.useTestRunsQuery.mock.calls.at(-1)?.[1];
    expect(params).not.toHaveProperty('since');
  });
});
