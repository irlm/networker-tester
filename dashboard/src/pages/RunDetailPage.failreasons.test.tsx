import { render, screen } from '@testing-library/react';
import { MemoryRouter, Route, Routes } from 'react-router';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { LiveAttempt, TestRun } from '../api/types';
import { RunDetailPage } from './RunDetailPage';

// #824: failed probe attempts rendered a bare red FAIL although the attempts
// API returns error_message for every one of them. The collapsed protocol
// block header must also surface the dominant failure reason
// ("5 FAIL — QUIC handshake timeout") so the collapsed view is diagnostic.

const mocks = vi.hoisted(() => ({
  useTestRunQuery: vi.fn(),
  useRunAttemptsQuery: vi.fn(),
  useRunArtifactQuery: vi.fn(),
  useRunInfraQuery: vi.fn(),
  useCancelRunMutation: vi.fn(),
  useComparisonGroupQuery: vi.fn(),
  useTestRunsQuery: vi.fn(),
  useTestConfigQuery: vi.fn(),
  usePinBaselineMutation: vi.fn(),
}));

vi.mock('../features/runs/queries', () => ({
  useTestRunQuery: mocks.useTestRunQuery,
  useRunAttemptsQuery: mocks.useRunAttemptsQuery,
  useRunArtifactQuery: mocks.useRunArtifactQuery,
  useRunInfraQuery: mocks.useRunInfraQuery,
  useCancelRunMutation: mocks.useCancelRunMutation,
  useComparisonGroupQuery: mocks.useComparisonGroupQuery,
  useTestRunsQuery: mocks.useTestRunsQuery,
  useTestConfigQuery: mocks.useTestConfigQuery,
  usePinBaselineMutation: mocks.usePinBaselineMutation,
}));
vi.mock('../hooks/useProject', () => ({
  useProject: () => ({ projectId: 'p-1', isProjectAdmin: false, isOperator: false }),
}));

const RUN_ID = 'a2d16c05-0000-4000-8000-000000000000';

function run(overrides: Partial<TestRun> = {}): TestRun {
  return {
    id: RUN_ID,
    test_config_id: 'cfg-1',
    project_id: 'p-1',
    status: 'completed',
    started_at: '2026-08-20T12:00:00Z',
    finished_at: '2026-08-20T12:05:00Z',
    success_count: 5,
    failure_count: 5,
    error_message: null,
    artifact_id: null,
    tester_id: null,
    worker_id: null,
    last_heartbeat: null,
    created_at: '2026-08-20T12:00:00Z',
    config_name: 'Full preset',
    comparison_group_id: null,
    ...overrides,
  };
}

function attempt(seq: number, protocol: string, overrides: Partial<LiveAttempt> = {}): LiveAttempt {
  return {
    attempt_id: `a-${protocol}-${seq}`,
    run_id: RUN_ID,
    protocol,
    sequence_num: seq,
    started_at: '2026-08-20T12:00:00Z',
    finished_at: '2026-08-20T12:00:01Z',
    success: true,
    retry_count: 0,
    ...overrides,
  };
}

// microsoft.com over h3, the run from the issue: every attempt fails with the
// same REST-shape flat error_message. http1 succeeds alongside it.
const h3Failures: LiveAttempt[] = [1, 2, 3, 4, 5].map((n) =>
  attempt(n, 'http3', { success: false, error_message: 'QUIC handshake timeout' }),
);
const h1Successes: LiveAttempt[] = [1, 2, 3, 4, 5].map((n) =>
  attempt(n, 'http1', {
    http: { status_code: 200, negotiated_version: 'HTTP/1.1', ttfb_ms: 20, total_duration_ms: 30 },
  }),
);

function mockAll(attempts: LiveAttempt[], current: TestRun = run()) {
  mocks.useTestRunQuery.mockReturnValue({ data: current, isPending: false, error: null, refetch: vi.fn() });
  mocks.useRunAttemptsQuery.mockReturnValue({ data: attempts, isPending: false, error: null, refetch: vi.fn() });
  mocks.useRunArtifactQuery.mockReturnValue({ data: null });
  mocks.useRunInfraQuery.mockReturnValue({ data: null });
  mocks.useCancelRunMutation.mockReturnValue({ mutate: vi.fn(), isPending: false });
  mocks.useComparisonGroupQuery.mockReturnValue({ data: null });
  mocks.useTestRunsQuery.mockReturnValue({ data: [], isPending: false });
  mocks.useTestConfigQuery.mockReturnValue({ data: null });
  mocks.usePinBaselineMutation.mockReturnValue({ mutate: vi.fn(), isPending: false });
}

function renderPage() {
  return render(
    <MemoryRouter initialEntries={[`/projects/p-1/runs/${RUN_ID}`]}>
      <Routes>
        <Route path="/projects/:projectId/runs/:runId" element={<RunDetailPage />} />
      </Routes>
    </MemoryRouter>,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
});

describe('RunDetailPage failure reasons (#824)', () => {
  it('appends the dominant reason to the collapsed protocol header when all failures share it', () => {
    mockAll([...h1Successes, ...h3Failures]);
    renderPage();

    const h3Header = screen.getByRole('button', { name: /HTTP3/ });
    expect(h3Header).toHaveTextContent('5 FAIL — QUIC handshake timeout');
    // Full line rides on the tooltip for when CSS truncates it.
    expect(
      screen.getByTitle('5 FAIL — QUIC handshake timeout'),
    ).toBeInTheDocument();
  });

  it('keeps a plain FAIL count when no single reason covers most failures', () => {
    const mixed = [
      attempt(1, 'http3', { success: false, error_message: 'QUIC handshake timeout' }),
      attempt(2, 'http3', { success: false, error_message: 'connection refused' }),
      attempt(3, 'http3', { success: false, error_message: 'dns lookup failed' }),
      attempt(4, 'http3', { success: false, error_message: 'connection reset' }),
    ];
    mockAll(mixed, run({ success_count: 0, failure_count: 4 }));
    renderPage();

    const h3Header = screen.getByRole('button', { name: /HTTP3/ });
    expect(h3Header).toHaveTextContent('4 FAIL');
    expect(h3Header).not.toHaveTextContent('—');
  });

  it('does not regress the all-success header: OK count only, no FAIL span', () => {
    mockAll(h1Successes, run({ success_count: 5, failure_count: 0 }));
    renderPage();

    const h1Header = screen.getByRole('button', { name: /HTTP1/ });
    expect(h1Header).toHaveTextContent('5 OK');
    expect(h1Header).not.toHaveTextContent('FAIL');
  });

  it('a majority reason wins the header even when a minority failed differently', () => {
    const mostlyQuic = [
      ...[1, 2, 3, 4].map((n) =>
        attempt(n, 'http3', { success: false, error_message: 'QUIC handshake timeout' }),
      ),
      attempt(5, 'http3', { success: false, error_message: 'connection refused' }),
    ];
    mockAll(mostlyQuic, run({ success_count: 0, failure_count: 5 }));
    renderPage();

    const h3Header = screen.getByRole('button', { name: /HTTP3/ });
    expect(h3Header).toHaveTextContent('5 FAIL — QUIC handshake timeout');
  });
});
