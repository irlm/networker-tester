import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter } from 'react-router';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { TestRun } from '../api/types';
import { RunsPage } from './RunsPage';

interface GroupQueryStub {
  data?: { id: string; name: string; cells: Array<{ label: string }> };
  isError: boolean;
}

const mocks = vi.hoisted(() => ({
  useTestRunsQuery: vi.fn(),
  useComparisonGroupsQueries: vi.fn(
    (groupIds: string[]): GroupQueryStub[] => groupIds.map(() => ({ data: undefined, isError: false })),
  ),
  refetch: vi.fn(),
}));

vi.mock('../features/runs/queries', () => ({
  useTestRunsQuery: mocks.useTestRunsQuery,
  useComparisonGroupsQueries: mocks.useComparisonGroupsQueries,
  runKeys: {
    detail: (runId: string) => ['runs', 'detail', runId],
    attempts: (runId: string) => ['runs', 'detail', runId, 'attempts'],
  },
}));
vi.mock('../hooks/useProject', () => ({ useProject: () => ({ projectId: 'project-1' }) }));
vi.mock('../hooks/useRenderLog', () => ({ useRenderLog: () => vi.fn() }));

// Anchored to the wall clock: the "Last 24 hours" filter test broke a day
// after this fixture's hard-coded 2026-08-18 timestamps aged out.
const anHourAgo = new Date(Date.now() - 60 * 60 * 1000).toISOString();

const baseRun = {
  project_id: 'project-1',
  status: 'completed',
  started_at: anHourAgo,
  finished_at: anHourAgo,
  success_count: 2,
  failure_count: 0,
  error_message: null,
  artifact_id: null,
  tester_id: null,
  worker_id: null,
  last_heartbeat: null,
  created_at: anHourAgo,
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

// ── Comparison-group rows (#803) ──────────────────────────────────────────────

const GROUP_ID = 'deadbeef-0000-4000-8000-000000000000';

const groupCellRuns: TestRun[] = [
  {
    ...baseRun,
    id: '55555555-5555-4555-8555-555555555555',
    test_config_id: 'config-cell-go',
    config_name: 'azure/eastus linux · nginx · cg-deadbeef·0·ab12',
    endpoint_kind: 'runtime',
    test_kind: 'benchmark',
    modes: ['apibench'],
    comparison_group_id: GROUP_ID,
  },
  {
    ...baseRun,
    id: '66666666-6666-4666-8666-666666666666',
    test_config_id: 'config-cell-py',
    config_name: 'azure/eastus linux · caddy · cg-deadbeef·1·ab12',
    endpoint_kind: 'runtime',
    test_kind: 'benchmark',
    modes: ['apibench'],
    comparison_group_id: GROUP_ID,
    status: 'running',
  },
];

describe('RunsPage comparison-group rows', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.useTestRunsQuery.mockReturnValue({
      data: [...groupCellRuns, ...runs],
      isPending: false,
      isError: false,
      dataUpdatedAt: Date.now(),
      refetch: mocks.refetch,
    });
    mocks.useComparisonGroupsQueries.mockImplementation((groupIds: string[]) =>
      groupIds.map(() => ({
        data: {
          id: GROUP_ID,
          name: 'nginx vs caddy',
          cells: Array.from({ length: 14 }, (_, i) => ({ label: `cell-${i}` })),
        },
        isError: false,
      })),
    );
  });

  it('collapses group cells into one row: name from the group API, X/N progress, running chip', () => {
    renderPage();

    const groupLink = screen.getByRole('link', { name: 'nginx vs caddy' });
    expect(groupLink).toHaveAttribute('href', `/projects/project-1/benchmarks/compare/${GROUP_ID}`);
    // Cells are hidden until expanded.
    expect(screen.queryByText(/nginx · cg-deadbeef/)).not.toBeInTheDocument();
    // Progress: 1 terminal of 14 defined cells (authoritative count, no "+").
    expect(screen.getByText(/1\/14/)).toBeInTheDocument();
    // One cell still running → aggregate chip is running.
    expect(screen.getByText('running')).toBeInTheDocument();
    // Standalone runs render exactly as before, as siblings.
    expect(screen.getByText('Checkout connectivity')).toBeInTheDocument();
  });

  it('expanding the group reveals the cell rows; view-as-list applies the group filter', async () => {
    const user = userEvent.setup();
    renderPage();

    await user.click(screen.getByRole('button', { name: /Expand group nginx vs caddy/ }));

    expect(screen.getByText('azure/eastus linux · nginx · cg-deadbeef·0·ab12')).toBeInTheDocument();
    expect(screen.getByText('azure/eastus linux · caddy · cg-deadbeef·1·ab12')).toBeInTheDocument();
    // Individual run URLs unchanged.
    expect(screen.getByRole('link', { name: '55555555' })).toHaveAttribute(
      'href',
      '/projects/project-1/runs/55555555-5555-4555-8555-555555555555',
    );
    expect(screen.getByRole('link', { name: 'view as list' })).toHaveAttribute(
      'href',
      `/projects/project-1/runs?comparison_group=${GROUP_ID}`,
    );
  });

  it('falls back to in-window counts with a "+" and a prefix-derived name when the group API 404s', () => {
    mocks.useComparisonGroupsQueries.mockImplementation((groupIds: string[]) =>
      groupIds.map(() => ({ data: undefined, isError: true })),
    );
    renderPage();

    // Deleted group (SET NULL): 1 terminal of the 2 visible cells, at least.
    expect(screen.getByText(/1\/2\+/)).toBeInTheDocument();
    // Name degrades to the cells' shared config-name prefix.
    expect(screen.getByRole('link', { name: 'azure/eastus linux' })).toBeInTheDocument();
  });

  it('the ?comparison_group= filter view stays a flat list (no group row)', () => {
    renderPage(`/projects/project-1/runs?comparison_group=${GROUP_ID}`);

    expect(screen.queryByRole('button', { name: /Expand group/ })).not.toBeInTheDocument();
    expect(screen.getByText('azure/eastus linux · nginx · cg-deadbeef·0·ab12')).toBeInTheDocument();
    expect(screen.getByText('azure/eastus linux · caddy · cg-deadbeef·1·ab12')).toBeInTheDocument();
  });
});
