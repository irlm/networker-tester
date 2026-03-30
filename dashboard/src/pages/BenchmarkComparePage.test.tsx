import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { BenchmarkComparisonReport } from '../api/types';
import { BenchmarkComparePage } from './BenchmarkComparePage';

const compareBenchmarksMock = vi.fn();
const getBenchmarkComparePresetsMock = vi.fn();
const saveBenchmarkComparePresetMock = vi.fn();
const deleteBenchmarkComparePresetMock = vi.fn();

vi.mock('../api/client', () => ({
  api: {
    compareBenchmarks: (...args: unknown[]) => compareBenchmarksMock(...args),
    getBenchmarkComparePresets: (...args: unknown[]) => getBenchmarkComparePresetsMock(...args),
    saveBenchmarkComparePreset: (...args: unknown[]) =>
      saveBenchmarkComparePresetMock(...args),
    deleteBenchmarkComparePreset: (...args: unknown[]) =>
      deleteBenchmarkComparePresetMock(...args),
  },
}));

function makeComparisonReport(
  overrides: Partial<BenchmarkComparisonReport> = {},
): BenchmarkComparisonReport {
  return {
    baseline_run_id: 'run-alpha',
    comparability_policy:
      'Only runs with matching client/server fingerprint, network type, and baseline RTT are compared.',
    gated_candidate_count: 1,
    runs: [
      {
        run_id: 'run-alpha',
        generated_at: '2026-03-30T12:00:00.000Z',
        target_host: 'alpha.example.com',
        scenario: 'warm',
        primary_phase: 'measured',
        phase_model: 'pilot-driven',
        publication_ready: true,
        noise_level: 'low',
        sufficiency: 'adequate',
        warning_count: 0,
        environment: {
          client_os: 'macos',
          client_arch: 'arm64',
          client_cpu_cores: 8,
          client_region: 'local',
          server_os: 'linux',
          server_arch: 'x86_64',
          server_cpu_cores: 4,
          server_region: 'us-east-1',
          network_type: 'loopback',
          baseline_rtt_p50_ms: 0.25,
          baseline_rtt_p95_ms: 0.4,
        },
      },
      {
        run_id: 'run-beta',
        generated_at: '2026-03-30T12:05:00.000Z',
        target_host: 'beta.example.com',
        scenario: 'warm',
        primary_phase: 'measured',
        phase_model: 'pilot-driven',
        publication_ready: false,
        noise_level: 'medium',
        sufficiency: 'marginal',
        warning_count: 2,
        environment: {
          client_os: 'macos',
          client_arch: 'arm64',
          client_cpu_cores: 8,
          client_region: 'local',
          server_os: 'linux',
          server_arch: 'x86_64',
          server_cpu_cores: 4,
          server_region: 'eu-west-1',
          network_type: 'wan',
          baseline_rtt_p50_ms: 12,
          baseline_rtt_p95_ms: 15,
        },
      },
    ],
    cases: [
      {
        case_id: 'download:1m',
        protocol: 'http1',
        payload_bytes: 1048576,
        http_stack: 'reqwest',
        metric_name: 'throughput',
        metric_unit: 'MB/s',
        higher_is_better: true,
        baseline: {
          run_id: 'run-alpha',
          generated_at: '2026-03-30T12:00:00.000Z',
          target_host: 'alpha.example.com',
          scenario: 'warm',
          primary_phase: 'measured',
          phase_model: 'pilot-driven',
          publication_ready: true,
          noise_level: 'low',
          sufficiency: 'adequate',
          warning_count: 0,
          included_sample_count: 50,
          failure_count: 0,
          error_count: 0,
          rps: 100,
          p95: 4200,
          p99: 4500,
          environment: {
            client_os: 'macos',
            client_arch: 'arm64',
            client_cpu_cores: 8,
            client_region: 'local',
            server_os: 'linux',
            server_arch: 'x86_64',
            server_cpu_cores: 4,
            server_region: 'us-east-1',
            network_type: 'loopback',
            baseline_rtt_p50_ms: 0.25,
            baseline_rtt_p95_ms: 0.4,
          },
          distribution: {
            sample_count: 50,
            min: 3800,
            mean: 4000,
            median: 4050,
            p95: 4200,
            p99: 4500,
            max: 4600,
            stddev: 200,
            cv: 0.05,
            standard_error: 20,
            ci95_lower: 3980,
            ci95_upper: 4120,
          },
        },
        candidates: [
          {
            run: {
              run_id: 'run-beta',
              generated_at: '2026-03-30T12:05:00.000Z',
              target_host: 'beta.example.com',
              scenario: 'warm',
              primary_phase: 'measured',
              phase_model: 'pilot-driven',
              publication_ready: false,
              noise_level: 'medium',
              sufficiency: 'marginal',
              warning_count: 2,
              included_sample_count: 50,
              failure_count: 0,
              error_count: 0,
              rps: 55,
              p95: 800,
              p99: 900,
              environment: {
                client_os: 'macos',
                client_arch: 'arm64',
                client_cpu_cores: 8,
                client_region: 'local',
                server_os: 'linux',
                server_arch: 'x86_64',
                server_cpu_cores: 4,
                server_region: 'eu-west-1',
                network_type: 'wan',
                baseline_rtt_p50_ms: 12,
                baseline_rtt_p95_ms: 15,
              },
              distribution: {
                sample_count: 50,
                min: 600,
                mean: 700,
                median: 680,
                p95: 800,
                p99: 900,
                max: 920,
                stddev: 60,
                cv: 0.08,
                standard_error: 8,
                ci95_lower: 650,
                ci95_upper: 710,
              },
            },
            comparable: false,
            comparability_notes: ['server region differs', 'network type differs'],
            absolute_delta: null,
            percent_delta: null,
            ratio: null,
            verdict: 'gated',
          },
        ],
      },
    ],
    ...overrides,
  };
}

function renderComparePage(initialEntry = '/projects/project-1/benchmarks/compare?runs=run-alpha,run-beta') {
  return render(
    <MemoryRouter initialEntries={[initialEntry]}>
      <Routes>
        <Route
          path="/projects/:projectId/benchmarks/compare"
          element={<BenchmarkComparePage />}
        />
      </Routes>
    </MemoryRouter>,
  );
}

describe('BenchmarkComparePage', () => {
  beforeEach(() => {
    compareBenchmarksMock.mockReset();
    getBenchmarkComparePresetsMock.mockReset();
    saveBenchmarkComparePresetMock.mockReset();
    deleteBenchmarkComparePresetMock.mockReset();
    getBenchmarkComparePresetsMock.mockResolvedValue([]);
    saveBenchmarkComparePresetMock.mockImplementation(async (_projectId, preset) => [
      {
        id: (preset as { id?: string }).id ?? 'preset-current',
        name: (preset as { name: string }).name,
        createdAt: '2026-03-30T12:10:00.000Z',
        updatedAt: '2026-03-30T12:10:00.000Z',
        runIds: (preset as { runIds: string[] }).runIds,
        baselineRunId: (preset as { baselineRunId: string | null }).baselineRunId,
        filters: (preset as { filters?: Record<string, string> }).filters,
      },
    ]);
    deleteBenchmarkComparePresetMock.mockResolvedValue([]);
  });

  it('renders gated comparisons with explicit comparability notes', async () => {
    compareBenchmarksMock.mockResolvedValue(makeComparisonReport());

    renderComparePage();

    expect(await screen.findByText('Saved Compare Presets')).toBeInTheDocument();
    expect(screen.getByText(/Only runs with matching client\/server fingerprint/)).toBeInTheDocument();
    expect(screen.getAllByText('gated').length).toBeGreaterThan(0);
    expect(screen.getByText('server region differs · network type differs')).toBeInTheDocument();
  });

  it('saves the current compare set and can apply another preset', async () => {
    const initialReport = makeComparisonReport();
    const sharedPreset = {
      id: 'preset-shared',
      name: 'Shared compare',
      createdAt: '2026-03-30T12:10:00.000Z',
      updatedAt: '2026-03-30T12:10:00.000Z',
      runIds: ['run-gamma', 'run-delta'],
      baselineRunId: 'run-gamma',
      filters: {
        targetSearch: '',
        scenario: 'warm',
        phaseModel: 'pilot-driven',
        serverRegion: 'us-east-1',
        networkType: 'loopback',
      },
    } as const;
    const appliedReport = makeComparisonReport({
      baseline_run_id: 'run-gamma',
      gated_candidate_count: 0,
      runs: [
        {
          ...initialReport.runs[0],
          run_id: 'run-gamma',
          target_host: 'gamma.example.com',
        },
        {
          ...initialReport.runs[1],
          run_id: 'run-delta',
          target_host: 'delta.example.com',
          environment: {
            ...initialReport.runs[1].environment,
            server_region: 'us-east-1',
            network_type: 'loopback',
            baseline_rtt_p50_ms: 0.3,
          },
        },
      ],
      cases: [
        {
          ...initialReport.cases[0],
          baseline: {
            ...initialReport.cases[0].baseline,
            run_id: 'run-gamma',
            target_host: 'gamma.example.com',
          },
          candidates: [
            {
              ...initialReport.cases[0].candidates[0],
              run: {
                ...initialReport.cases[0].candidates[0].run,
                run_id: 'run-delta',
                target_host: 'delta.example.com',
                environment: {
                  ...initialReport.cases[0].candidates[0].run.environment,
                  server_region: 'us-east-1',
                  network_type: 'loopback',
                  baseline_rtt_p50_ms: 0.3,
                },
              },
              comparable: true,
              comparability_notes: [],
              absolute_delta: 10,
              percent_delta: 2.5,
              ratio: 1.02,
              verdict: 'better',
            },
          ],
        },
      ],
    });

    compareBenchmarksMock
      .mockResolvedValueOnce(initialReport)
      .mockResolvedValueOnce(appliedReport);

    getBenchmarkComparePresetsMock.mockResolvedValueOnce([sharedPreset]);
    saveBenchmarkComparePresetMock.mockImplementationOnce(async (_projectId, preset) => [
      sharedPreset,
      {
        id: (preset as { id?: string }).id ?? 'preset-current',
        name: (preset as { name: string }).name,
        createdAt: '2026-03-30T12:10:00.000Z',
        updatedAt: '2026-03-30T12:10:00.000Z',
        runIds: (preset as { runIds: string[] }).runIds,
        baselineRunId: (preset as { baselineRunId: string | null }).baselineRunId,
        filters: (preset as { filters?: Record<string, string> }).filters,
      },
    ]);

    renderComparePage();

    expect(await screen.findByText('Shared compare')).toBeInTheDocument();
    await userEvent.type(screen.getByLabelText('Preset name'), 'Current compare');
    await userEvent.click(screen.getByRole('button', { name: 'Save current compare' }));

    expect(await screen.findByText('Saved preset Current compare.')).toBeInTheDocument();
    expect(screen.getByText('Current compare')).toBeInTheDocument();

    const sharedPresetCard = screen
      .getByText('Shared compare')
      .closest('div.rounded-lg');
    expect(sharedPresetCard).not.toBeNull();
    await userEvent.click(
      within(sharedPresetCard as HTMLElement).getByRole('button', { name: 'Apply' }),
    );

    await waitFor(() => {
      expect(compareBenchmarksMock).toHaveBeenLastCalledWith(
        'project-1',
        ['run-gamma', 'run-delta'],
        'run-gamma',
      );
    });
    expect(await screen.findByText('gamma.example.com')).toBeInTheDocument();
  });
});
