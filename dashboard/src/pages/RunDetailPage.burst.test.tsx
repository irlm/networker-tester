import { render, screen, within } from '@testing-library/react';
import { MemoryRouter, Route, Routes } from 'react-router';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { LiveAttempt, TestRun } from '../api/types';
import { RunDetailPage } from './RunDetailPage';

// #782 P2: burst sampling makes a point's median meaningful, and the run
// detail has to report it WITHOUT overclaiming. The rules under test:
//   * a bursted point shows its median as the headline with p95/min/max beside
//     it, and the outlier sample must not move the median;
//   * a point with too few usable samples is labelled a reading, not a median,
//     and shows no p95;
//   * a run with no repeats renders no median section at all.

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

const RUN_ID = 'b7d16c05-0000-4000-8000-000000000000';

function run(overrides: Partial<TestRun> = {}): TestRun {
  return {
    id: RUN_ID,
    test_config_id: 'cfg-1',
    project_id: 'p-1',
    status: 'completed',
    started_at: '2026-08-20T12:00:00Z',
    finished_at: '2026-08-20T12:05:00Z',
    success_count: 5,
    failure_count: 0,
    error_message: null,
    artifact_id: null,
    tester_id: null,
    worker_id: null,
    last_heartbeat: null,
    created_at: '2026-08-20T12:00:00Z',
    config_name: 'Diag: example.com (Quick x5)',
    comparison_group_id: null,
    ...overrides,
  };
}

function burstSample(sampleIndex: number, ms: number): LiveAttempt {
  return {
    attempt_id: `a-http2-${sampleIndex}`,
    run_id: RUN_ID,
    protocol: 'http2',
    sequence_num: sampleIndex,
    started_at: '2026-08-20T12:00:00Z',
    finished_at: '2026-08-20T12:00:01Z',
    success: true,
    retry_count: 0,
    sample_index: sampleIndex,
    target_url: 'https://example.com/',
    http: {
      status_code: 200,
      negotiated_version: 'HTTP/2.0',
      ttfb_ms: ms / 2,
      total_duration_ms: ms,
    },
  };
}

/** A mode whose primary metric the REST /attempts endpoint does not carry:
 *  browser1's number lives in `browser.load_ms`, and AttemptView has no
 *  `browser` block (no browser phase table exists), so a completed run loads
 *  these as successes with no metric. */
function metriclessSample(sampleIndex: number, success = true): LiveAttempt {
  return {
    attempt_id: `a-browser1-${sampleIndex}`,
    run_id: RUN_ID,
    protocol: 'browser1',
    sequence_num: sampleIndex,
    started_at: '2026-08-20T12:00:00Z',
    finished_at: '2026-08-20T12:00:01Z',
    success,
    retry_count: 0,
    sample_index: sampleIndex,
    target_url: 'https://example.com/',
    ...(success ? {} : { error_message: 'navigation failed' }),
  };
}

/** An h3 sample the tester DECLINED to run: the target advertises no
 *  `Alt-Svc: h3=`, so the pre-flight (v0.28.301) records `unsupported`
 *  instead of dispatching QUIC. Unsuccessful, but not a failure. */
function notOfferedSample(sampleIndex: number, viaRest = true): LiveAttempt {
  const base: LiveAttempt = {
    attempt_id: `a-http3-${sampleIndex}`,
    run_id: RUN_ID,
    protocol: 'http3',
    sequence_num: sampleIndex,
    started_at: '2026-08-20T12:00:00Z',
    finished_at: '2026-08-20T12:00:00Z',
    success: false,
    retry_count: 0,
    sample_index: sampleIndex,
    target_url: 'https://example.com/',
    error_message: 'http3 not run: the target advertises no HTTP/3',
  };
  // REST carries the class flat; the live stream nests it. Both must work.
  return viaRest
    ? { ...base, error_category: 'unsupported' }
    : { ...base, error: { category: 'unsupported', message: 'not offered' } };
}

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

/** The burst median/spread section, scoped so generic numbers elsewhere on the
 *  page (probe counts, the statistics summary) cannot satisfy an assertion. */
function burstSection(): HTMLElement {
  const heading = screen.getByText(/median & spread per point/i);
  const section = heading.parentElement;
  if (!section) throw new Error('burst section has no container');
  return section;
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

describe('RunDetailPage — burst sampling median & spread', () => {
  it('reports the median of a five-sample burst, not the outlier', () => {
    // 10 11 11 12 40 → median 11ms. A single 40ms sample must not become the
    // headline number; that is the entire point of the burst.
    mockAll([10, 12, 11, 40, 11].map((ms, i) => burstSample(i, ms)));
    renderPage();

    const section = burstSection();
    // 5 usable of 5 samples.
    expect(within(section).getByText('/5')).toBeInTheDocument();
    // Median headline + spread alongside, in that column order.
    const cells = within(section)
      .getAllByRole('cell')
      .map((c) => c.textContent?.trim());
    expect(cells).toEqual([
      'HTTP2',
      'Total ms',
      '5/5',
      '11.00ms', // median — the 40ms outlier did NOT move it
      '34.40ms', // p95 — but the tail IS reported, not hidden
      '10.00ms', // min
      '40.00ms', // max
      '3.13\u00D7', // p95/p50: this point is noisy, and says so
    ]);
    expect(within(section).queryByText(/1 sample\)/)).not.toBeInTheDocument();
  });

  it('says "1 sample" instead of calling one reading a median', () => {
    // Two points: a healthy burst and a mode that only produced one usable
    // sample. Only the second may be labelled.
    mockAll([
      ...[10, 12, 11].map((ms, i) => burstSample(i, ms)),
      {
        ...burstSample(0, 55),
        attempt_id: 'a-tcp-0',
        protocol: 'tcp',
        http: undefined,
        tcp: { connect_duration_ms: 55, remote_addr: '1.2.3.4:443' },
      },
      {
        ...burstSample(1, 0),
        attempt_id: 'a-tcp-1',
        protocol: 'tcp',
        success: false,
        http: undefined,
        error_message: 'connection refused',
      },
    ]);
    renderPage();

    expect(screen.getByText('(1 sample)')).toBeInTheDocument();
    expect(screen.getByText(/1 failed/)).toBeInTheDocument();
    expect(
      screen.getByText(/those numbers are readings, not medians/),
    ).toBeInTheDocument();
  });

  it('says every sample failed rather than showing a blank median', () => {
    // A URL that is entirely down in a set run: five samples ran, five
    // failed. The row must report that, not an empty median cell.
    mockAll([
      ...[10, 12, 11].map((ms, i) => burstSample(i, ms)),
      ...[0, 1].map((i) => ({
        ...burstSample(i, 0),
        attempt_id: `a-down-${i}`,
        target_url: 'https://down.example/',
        success: false,
        http: undefined,
        error_message: 'connection refused',
      })),
    ]);
    renderPage();

    const section = burstSection();
    expect(within(section).getByText(/no usable sample/)).toBeInTheDocument();
    expect(within(section).getByText(/2 failed/)).toBeInTheDocument();
    // The healthy URL still gets its median — one dead URL does not blank the
    // whole section.
    expect(within(section).getByText('11.00ms')).toBeInTheDocument();
  });

  // A success is not a failure. Three modes on the real run detail (TLSRESUME,
  // BROWSER1, BROWSER2) were 5/5 successful yet the row asserted "every sample
  // failed", because a null `stats` was read as total failure when it really
  // means "no usable metric". The two causes are distinct and both tested.
  it('does not claim failure when every sample succeeded but carried no metric', () => {
    mockAll([0, 1, 2, 3, 4].map((i) => metriclessSample(i)));
    renderPage();

    const section = burstSection();
    expect(within(section).queryByText(/every sample failed/)).not.toBeInTheDocument();
    expect(within(section).queryByText(/\d+ failed/)).not.toBeInTheDocument();
    expect(
      within(section).getByText(/no load ms recorded .* all 5 samples succeeded/),
    ).toBeInTheDocument();
  });

  it('separates the failed samples from the metric-less ones when both occur', () => {
    mockAll([
      ...[0, 1, 2].map((i) => metriclessSample(i)),
      ...[3, 4].map((i) => metriclessSample(i, false)),
    ]);
    renderPage();

    const section = burstSection();
    expect(within(section).queryByText(/every sample failed/)).not.toBeInTheDocument();
    expect(
      within(section).getByText(/2 failed, the rest reported no load ms/),
    ).toBeInTheDocument();
  });

  it('gives tlsresume a real median instead of treating it as metric-less', () => {
    // tlsresume reports the resumed handshake in `tls`, the same field as tls.
    // The TS metric map omitted it, dropping the mode to the http default that
    // a TLS-only probe never carries — so the row read as metric-less.
    mockAll(
      [8, 10, 9].map((ms, i) => ({
        ...burstSample(i, ms),
        attempt_id: `a-tlsresume-${i}`,
        protocol: 'tlsresume',
        http: undefined,
        tls: {
          handshake_duration_ms: ms,
          protocol_version: 'TLSv1.3',
          cipher_suite: 'TLS_AES_128_GCM_SHA256',
        },
      })),
    );
    renderPage();

    const section = burstSection();
    expect(within(section).queryByText(/no usable sample/)).not.toBeInTheDocument();
    expect(within(section).getByText('9.00ms')).toBeInTheDocument();
    expect(within(section).getByText(/handshake ms/i)).toBeInTheDocument();
  });

  // The h3 pre-flight only pays off if the UI stops calling a skipped sample a
  // failure. These pin that, for BOTH transports of the category.
  it('reports h3 as not offered rather than failed when the target has no HTTP/3', () => {
    mockAll([0, 1, 2, 3, 4].map((i) => notOfferedSample(i)));
    renderPage();

    const section = burstSection();
    expect(within(section).queryByText(/every sample failed/)).not.toBeInTheDocument();
    expect(within(section).queryByText(/\d+ failed/)).not.toBeInTheDocument();
    expect(within(section).getByText(/5 not offered/)).toBeInTheDocument();
    expect(
      within(section).getByText(/not run .* this target does not offer HTTP\/3/),
    ).toBeInTheDocument();
  });

  it('reads the category from the live stream shape too', () => {
    mockAll([0, 1, 2].map((i) => notOfferedSample(i, false)));
    renderPage();

    const section = burstSection();
    expect(within(section).queryByText(/\d+ failed/)).not.toBeInTheDocument();
    expect(within(section).getByText(/3 not offered/)).toBeInTheDocument();
  });

  it('keeps real failures separate from not-offered samples in one point', () => {
    // A genuinely broken h3 target: some samples skipped, some hard-failed.
    mockAll([
      ...[0, 1, 2].map((i) => notOfferedSample(i)),
      ...[3, 4].map((i) => ({
        ...notOfferedSample(i),
        error_category: 'tls',
        error_message: 'QUIC connect: aborted by peer',
      })),
    ]);
    renderPage();

    const section = burstSection();
    // "2 failed" appears in the Samples cell AND the summary cell; both are
    // correct, so assert presence rather than uniqueness.
    expect(within(section).getAllByText(/2 failed/).length).toBeGreaterThan(0);
    expect(within(section).getByText(/3 not offered/)).toBeInTheDocument();
    // Not ALL samples were skipped, so the not-offered headline must not win.
    expect(
      within(section).queryByText(/this target does not offer/),
    ).not.toBeInTheDocument();
  });

  it('renders no median section for a run whose points ran once each', () => {
    mockAll([burstSample(0, 10)]);
    renderPage();

    expect(screen.queryByText(/median & spread per point/i)).not.toBeInTheDocument();
  });
});
