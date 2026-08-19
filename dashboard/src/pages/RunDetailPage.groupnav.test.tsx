import { render, screen } from '@testing-library/react';
import { MemoryRouter, Route, Routes } from 'react-router';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { TestRun } from '../api/types';
import { RunDetailPage } from './RunDetailPage';

// #803: a comparison-group cell gets Runs / <group> / <run> breadcrumbs and
// prev/next navigation through its sibling cells, ordered by cell label.

const mocks = vi.hoisted(() => ({
  useTestRunQuery: vi.fn(),
  useRunAttemptsQuery: vi.fn(),
  useRunArtifactQuery: vi.fn(),
  useRunInfraQuery: vi.fn(),
  useCancelRunMutation: vi.fn(),
  useComparisonGroupQuery: vi.fn(),
  useTestRunsQuery: vi.fn(),
}));

vi.mock('../features/runs/queries', () => ({
  useTestRunQuery: mocks.useTestRunQuery,
  useRunAttemptsQuery: mocks.useRunAttemptsQuery,
  useRunArtifactQuery: mocks.useRunArtifactQuery,
  useRunInfraQuery: mocks.useRunInfraQuery,
  useCancelRunMutation: mocks.useCancelRunMutation,
  useComparisonGroupQuery: mocks.useComparisonGroupQuery,
  useTestRunsQuery: mocks.useTestRunsQuery,
}));
vi.mock('../hooks/useProject', () => ({
  useProject: () => ({ projectId: 'p-1', isProjectAdmin: false }),
}));

const GROUP_ID = 'cafecafe-0000-4000-8000-000000000000';

function run(id: string, configName: string, groupId: string | null = GROUP_ID): TestRun {
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
    comparison_group_id: groupId,
  };
}

// Deliberately NOT in label order — created order and label order differ.
const siblings = [
  run('run-charlie', `charlie cell · cg-cafecafe·2·ab12`),
  run('run-alpha', `alpha cell · cg-cafecafe·0·ab12`),
  run('run-bravo', `bravo cell · cg-cafecafe·1·ab12`),
];

function mockAll(current: TestRun) {
  mocks.useTestRunQuery.mockReturnValue({ data: current, isPending: false, error: null, refetch: vi.fn() });
  mocks.useRunAttemptsQuery.mockReturnValue({ data: [], isPending: false, error: null, refetch: vi.fn() });
  mocks.useRunArtifactQuery.mockReturnValue({ data: null });
  mocks.useRunInfraQuery.mockReturnValue({ data: null });
  mocks.useCancelRunMutation.mockReturnValue({ mutate: vi.fn(), isPending: false });
  mocks.useComparisonGroupQuery.mockReturnValue({ data: { id: GROUP_ID, name: 'my matrix' } });
  mocks.useTestRunsQuery.mockReturnValue({ data: siblings, isPending: false });
}

function renderPage(runId: string) {
  return render(
    <MemoryRouter initialEntries={[`/projects/p-1/runs/${runId}`]}>
      <Routes>
        <Route path="/projects/:projectId/runs/:runId" element={<RunDetailPage />} />
      </Routes>
    </MemoryRouter>,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
});

describe('RunDetailPage group context (#803)', () => {
  it('breadcrumbs Runs / <group> / <run>, with the group crumb linking to the compare page', () => {
    mockAll(siblings[2]); // bravo
    renderPage('run-bravo');

    const crumb = screen.getByRole('link', { name: 'my matrix' });
    expect(crumb).toHaveAttribute('href', `/projects/p-1/benchmarks/compare/${GROUP_ID}`);
    expect(screen.getByRole('link', { name: 'Runs' })).toHaveAttribute('href', '/projects/p-1/runs');
  });

  it('shows "cell X of N" with prev/next ordered by cell label, not created order', () => {
    mockAll(siblings[2]); // bravo — middle by label despite being created last
    renderPage('run-bravo');

    expect(
      screen.getByText((_, el) => el?.tagName === 'SPAN' && el.textContent === 'cell 2 of 3'),
    ).toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'Previous cell' })).toHaveAttribute(
      'href', '/projects/p-1/runs/run-alpha',
    );
    expect(screen.getByRole('link', { name: 'Next cell' })).toHaveAttribute(
      'href', '/projects/p-1/runs/run-charlie',
    );
  });

  it('the first cell has no prev link and the group name falls back when the group API 404s', () => {
    mockAll(siblings[1]); // alpha
    mocks.useComparisonGroupQuery.mockReturnValue({ data: undefined, isError: true });
    renderPage('run-alpha');

    expect(screen.queryByRole('link', { name: 'Previous cell' })).not.toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'Next cell' })).toHaveAttribute(
      'href', '/projects/p-1/runs/run-bravo',
    );
    // No shared prefix among 'alpha/bravo/charlie cell' → Group <shortid>.
    expect(screen.getByRole('link', { name: `Group ${GROUP_ID.slice(0, 8)}` })).toBeInTheDocument();
  });

  it('a standalone run keeps the two-level breadcrumb and no cell nav', () => {
    const standalone = run('run-solo', 'Solo run', null);
    mockAll(standalone);
    mocks.useTestRunsQuery.mockReturnValue({ data: undefined, isPending: false });
    renderPage('run-solo');

    expect(screen.queryByText(/cell \d+ of/)).not.toBeInTheDocument();
    expect(screen.queryByRole('link', { name: 'my matrix' })).not.toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'Runs' })).toBeInTheDocument();
  });
});
