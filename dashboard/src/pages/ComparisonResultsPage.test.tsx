import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter, Route, Routes } from 'react-router';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { BenchmarkArtifact, ComparisonGroup, LiveAttempt, TestRun } from '../api/types';
import { ComparisonResultsPage } from './ComparisonResultsPage';

const mocks = vi.hoisted(() => ({
  useTestRunsQuery: vi.fn(),
  useRunsAttemptsQueries: vi.fn(),
  useRunsArtifactsQueries: vi.fn(),
  useComparisonGroupQuery: vi.fn(),
  deleteMutate: vi.fn(),
}));

vi.mock('../features/runs/queries', () => ({
  useTestRunsQuery: mocks.useTestRunsQuery,
  useRunsAttemptsQueries: mocks.useRunsAttemptsQueries,
  useRunsArtifactsQueries: mocks.useRunsArtifactsQueries,
  useComparisonGroupQuery: mocks.useComparisonGroupQuery,
  useDeleteComparisonGroupMutation: () => ({ mutate: mocks.deleteMutate, isPending: false }),
}));
vi.mock('../hooks/useProject', () => ({ useProject: () => ({ projectId: 'p-1', isOperator: true }) }));

const GROUP_ID = '485406f0-0000-4000-8000-000000000000';

function run(id: string, configName: string, overrides: Partial<TestRun> = {}): TestRun {
  return {
    id,
    test_config_id: `cfg-${id}`,
    project_id: 'p-1',
    status: 'completed',
    started_at: '2026-08-18T12:00:00Z',
    finished_at: '2026-08-18T12:05:00Z',
    success_count: 4,
    failure_count: 0,
    error_message: null,
    artifact_id: null,
    tester_id: null,
    worker_id: null,
    last_heartbeat: null,
    created_at: '2026-08-18T12:00:00Z',
    config_name: configName,
    comparison_group_id: GROUP_ID,
    ...overrides,
  };
}

function httpAttempt(totalMs: number, success = true): LiveAttempt {
  return {
    attempt_id: `a-${Math.random()}`,
    run_id: 'r',
    protocol: 'http1',
    sequence_num: 0,
    started_at: '2026-08-18T12:00:00Z',
    finished_at: '2026-08-18T12:00:01Z',
    success,
    retry_count: 0,
    http: success
      ? { status_code: 200, ttfb_ms: totalMs / 2, total_duration_ms: totalMs, negotiated_version: 'HTTP/1.1' }
      : undefined,
  };
}

function attemptsAround(p50: number): LiveAttempt[] {
  return [httpAttempt(p50 - 10), httpAttempt(p50), httpAttempt(p50), httpAttempt(p50 + 10)];
}

const suffix = (i: number) => ` · cg-485406f0·${i}·ab12`;

const runs: TestRun[] = [
  run('run-go-nginx', `go @ azure/eastus @ linux @ nginx${suffix(0)}`),
  run('run-go-caddy', `go @ azure/eastus @ linux @ caddy${suffix(1)}`),
  run('run-py-nginx', `python @ azure/eastus @ linux @ nginx${suffix(2)}`, { artifact_id: 'art-1' }),
  run('run-py-caddy', `python @ azure/eastus @ linux @ caddy${suffix(3)}`),
];

const attemptsByRun: Record<string, LiveAttempt[]> = {
  'run-go-nginx': attemptsAround(100),
  'run-go-caddy': attemptsAround(120),
  'run-py-nginx': attemptsAround(200),
  'run-py-caddy': attemptsAround(260),
};

const group: ComparisonGroup = {
  id: GROUP_ID,
  project_id: 'p-1',
  name: 'go vs python',
  base_workload: { modes: ['apibench'], runs: 60 } as ComparisonGroup['base_workload'],
  cells: runs.map((r, i) => ({
    label: (r.config_name ?? '').replace(suffix(i), ''),
    endpoint: {
      kind: 'pending',
      cloud_account_id: 'acc-1',
      region: 'eastus',
      vm_size: 'Standard_B2s',
      os: 'linux',
      proxy_stack: i % 2 === 0 ? 'nginx' : 'caddy',
      topology: 'loopback',
    },
  })),
  status: 'completed',
  created_at: '2026-08-18T11:55:00Z',
};

const pyNginxArtifact = {
  cases: [
    { id: 'sort', protocol: 'http1', payload_bytes: null, http_stack: null, metric_name: 'latency', metric_unit: 'ms', higher_is_better: false },
  ],
  summaries: [
    {
      case_id: 'sort', protocol: 'http1', payload_bytes: null, http_stack: null,
      metric_name: 'latency', metric_unit: 'ms', higher_is_better: false,
      sample_count: 15, included_sample_count: 15, success_count: 15, failure_count: 0,
      p50: 3.2, p95: 7.9,
    },
  ],
} as unknown as BenchmarkArtifact;

function mockQueries({ artifacts = {} as Record<string, BenchmarkArtifact> } = {}) {
  mocks.useTestRunsQuery.mockReturnValue({ data: runs, isPending: false, error: null });
  mocks.useRunsAttemptsQueries.mockImplementation((runIds: string[]) =>
    runIds.map((id) => ({ data: attemptsByRun[id] ?? [], isLoading: false })),
  );
  mocks.useRunsArtifactsQueries.mockImplementation((rows: TestRun[]) =>
    rows.map((r) => ({ data: artifacts[r.id] ?? null })),
  );
  mocks.useComparisonGroupQuery.mockReturnValue({ data: group });
}

function renderPage() {
  return render(
    <MemoryRouter initialEntries={[`/projects/p-1/benchmarks/compare/${GROUP_ID}`]}>
      <Routes>
        <Route
          path="/projects/:projectId/benchmarks/compare/:groupId"
          element={<ComparisonResultsPage />}
        />
      </Routes>
    </MemoryRouter>,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  mockQueries();
});

describe('ComparisonResultsPage', () => {
  it('renders the group header with cell count and status chips', () => {
    renderPage();
    expect(screen.getByText('go vs python')).toBeInTheDocument();
    expect(screen.getByText('4 completed')).toBeInTheDocument();
    expect(screen.getByText(/4 cells/)).toBeInTheDocument();
    expect(screen.getByText('485406f0')).toBeInTheDocument();
  });

  it('defaults to the by-testbed pivot with one section per environment, ranked fastest first', () => {
    renderPage();

    const nginxSection = screen
      .getByRole('heading', { name: /azure\/eastus · linux · nginx/ })
      .closest('section')!;
    const rows = within(nginxSection).getAllByRole('row').slice(1); // drop header
    expect(within(rows[0]).getByText('go')).toBeInTheDocument();
    expect(within(rows[0]).getByText('fastest')).toBeInTheDocument();
    expect(within(rows[1]).getByText('python')).toBeInTheDocument();

    expect(
      screen.getByRole('heading', { name: /azure\/eastus · linux · caddy/ }),
    ).toBeInTheDocument();
  });

  it('links every cell to its run detail page', () => {
    renderPage();
    const link = screen.getAllByRole('link', { name: /run run-go-n/ })[0];
    expect(link).toHaveAttribute('href', '/projects/p-1/runs/run-go-nginx');
  });

  it('notes the missing per-case breakdown when artifacts carry no cases (#796)', () => {
    renderPage();
    expect(screen.getAllByText(/Per-case breakdown unavailable/).length).toBeGreaterThan(0);
  });

  it('renders the per-case table when an artifact has cases, noting the runs without', () => {
    mockQueries({ artifacts: { 'run-py-nginx': pyNginxArtifact } });
    renderPage();

    const nginxSection = screen
      .getByRole('heading', { name: /azure\/eastus · linux · nginx/ })
      .closest('section')!;
    expect(within(nginxSection).getByText('sort')).toBeInTheDocument();
    expect(within(nginxSection).getByText('3.20 ms')).toBeInTheDocument();
    // The go cell in the same section has no per-case rows → named in the note.
    expect(within(nginxSection).getByText(/Per-case breakdown unavailable for/)).toBeInTheDocument();
  });

  it('switches to the by-language pivot with variant labels, deltas, and the fairness flag off for one axis', async () => {
    const user = userEvent.setup();
    renderPage();
    await user.click(screen.getByRole('button', { name: 'By language' }));

    const goHeading = screen.getByRole('heading', { name: 'go varying: proxy' });
    const goSection = goHeading.closest('section')!;
    expect(within(goSection).queryByText(/multiple variables differ/)).not.toBeInTheDocument();

    const goRows = within(goSection).getAllByRole('row').slice(1);
    expect(within(goRows[0]).getByText('nginx')).toBeInTheDocument();
    expect(within(goRows[0]).getByText('fastest')).toBeInTheDocument();
    expect(within(goRows[1]).getByText('caddy')).toBeInTheDocument();
    expect(within(goRows[1]).getByText('+20.0%')).toBeInTheDocument();

    // python: 260 vs 200 → +30%
    const pySection = screen
      .getByRole('heading', { name: 'python varying: proxy' })
      .closest('section')!;
    expect(within(pySection).getByText('+30.0%')).toBeInTheDocument();
  });

  it('flags multi-variable language groups instead of implying causality', async () => {
    const multiRuns = [
      run('run-go-lnx', `go @ azure/eastus @ linux @ nginx${suffix(0)}`),
      run('run-go-win', `go @ azure/eastus @ windows @ iis${suffix(1)}`),
    ];
    mocks.useTestRunsQuery.mockReturnValue({ data: multiRuns, isPending: false, error: null });
    mocks.useRunsAttemptsQueries.mockImplementation((runIds: string[]) =>
      runIds.map((id) => ({ data: id === 'run-go-lnx' ? attemptsAround(100) : attemptsAround(180), isLoading: false })),
    );
    mocks.useRunsArtifactsQueries.mockImplementation((rows: TestRun[]) => rows.map(() => ({ data: null })));
    mocks.useComparisonGroupQuery.mockReturnValue({ data: null });

    const user = userEvent.setup();
    renderPage();
    await user.click(screen.getByRole('button', { name: 'By language' }));

    expect(screen.getByText(/multiple variables differ \(os · proxy\)/)).toBeInTheDocument();
    expect(screen.getByText('+80.0%')).toBeInTheDocument();
  });

  it('shows the empty state when the group has no runs', () => {
    mocks.useTestRunsQuery.mockReturnValue({ data: [], isPending: false, error: null });
    mocks.useRunsAttemptsQueries.mockImplementation((runIds: string[]) => runIds.map(() => ({ data: [] })));
    mocks.useRunsArtifactsQueries.mockImplementation((rows: TestRun[]) => rows.map(() => ({ data: null })));
    renderPage();
    expect(screen.getByText(/No runs found for this comparison group/)).toBeInTheDocument();
  });

  // ── #803: completion banner + group actions ─────────────────────────────────

  it('shows the completion banner with failed cells listed by label and ANSI-stripped reason', () => {
    const withFailure = [
      ...runs.slice(0, 3),
      run('run-py-caddy', `python @ azure/eastus @ linux @ caddy${suffix(3)}`, {
        status: 'failed',
        success_count: 0,
        error_message: '\u001b[31mprovisioning timed out\u001b[0m',
      }),
    ];
    mocks.useTestRunsQuery.mockReturnValue({ data: withFailure, isPending: false, error: null });
    renderPage();

    const banner = screen.getByRole('status');
    expect(banner.textContent).toContain('3/4 completed');
    expect(banner.textContent).toContain('1 failed');
    expect(within(banner).getByText('python @ azure/eastus @ linux @ caddy')).toBeInTheDocument();
    expect(banner.textContent).toContain('provisioning timed out');
    expect(banner.textContent).not.toContain('\u001b[31m');
    expect(within(banner).getByRole('link', { name: /run run-py-c/ })).toHaveAttribute(
      'href', '/projects/p-1/runs/run-py-caddy',
    );
  });

  it('counts N from the group definition, so missing cells are visible (12 defined, 4 fetched)', () => {
    mocks.useComparisonGroupQuery.mockReturnValue({
      data: { ...group, cells: Array.from({ length: 12 }, (_, i) => group.cells[i % 4]) },
    });
    renderPage();
    expect(screen.getByRole('status').textContent).toContain('4/12 completed');
  });

  it('deletes the group after confirmation', async () => {
    const user = userEvent.setup();
    renderPage();

    await user.click(screen.getByRole('button', { name: 'Delete group' }));
    const dialog = screen.getByRole('alertdialog');
    expect(dialog.textContent).toContain('NOT');
    await user.click(within(dialog).getByRole('button', { name: 'Delete group' }));

    expect(mocks.deleteMutate).toHaveBeenCalledWith(GROUP_ID, expect.anything());
  });
});
