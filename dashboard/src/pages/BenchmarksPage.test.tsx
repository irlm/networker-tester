import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { BenchmarkRunSummary } from '../api/types';
import { BenchmarksPage } from './BenchmarksPage';

const getBenchmarksMock = vi.fn();
const getBenchmarkComparePresetsMock = vi.fn();
const saveBenchmarkComparePresetMock = vi.fn();
const deleteBenchmarkComparePresetMock = vi.fn();

vi.mock('../api/client', () => ({
  api: {
    getBenchmarks: (...args: unknown[]) => getBenchmarksMock(...args),
    getBenchmarkComparePresets: (...args: unknown[]) => getBenchmarkComparePresetsMock(...args),
    saveBenchmarkComparePreset: (...args: unknown[]) =>
      saveBenchmarkComparePresetMock(...args),
    deleteBenchmarkComparePreset: (...args: unknown[]) =>
      deleteBenchmarkComparePresetMock(...args),
  },
}));

function makeBenchmarkRunSummary(
  runId: string,
  overrides: Partial<BenchmarkRunSummary> = {},
): BenchmarkRunSummary {
  return {
    run_id: runId,
    generated_at: '2026-03-30T12:00:00.000Z',
    target_url: 'http://alpha.example.com/health',
    target_host: 'alpha.example.com',
    modes: ['download'],
    concurrency: 1,
    total_runs: 50,
    contract_version: '1.2',
    scenario: 'warm',
    primary_phase: 'measured',
    phase_model: 'environment-check>stability-check>pilot>overhead>measured>cooldown',
    execution_plan_source: 'pilot',
    server_region: 'us-east-1',
    network_type: 'loopback',
    baseline_rtt_p50_ms: 0.25,
    total_cases: 1,
    total_samples: 50,
    publication_ready: true,
    noise_level: 'low',
    sufficiency: 'adequate',
    publication_blocker_count: 0,
    warnings: [],
    ...overrides,
  };
}

function renderBenchmarksPage() {
  return render(
    <MemoryRouter initialEntries={['/projects/project-1/benchmarks']}>
      <Routes>
        <Route path="/projects/:projectId/benchmarks" element={<BenchmarksPage />} />
      </Routes>
    </MemoryRouter>,
  );
}

describe('BenchmarksPage', () => {
  beforeEach(() => {
    getBenchmarksMock.mockReset();
    getBenchmarkComparePresetsMock.mockReset();
    saveBenchmarkComparePresetMock.mockReset();
    deleteBenchmarkComparePresetMock.mockReset();
    getBenchmarkComparePresetsMock.mockResolvedValue([]);
    saveBenchmarkComparePresetMock.mockImplementation(async (_projectId, preset) => [
      {
        id: 'preset-warm-compare',
        name: (preset as { name: string }).name,
        createdAt: '2026-03-30T12:20:00.000Z',
        updatedAt: '2026-03-30T12:20:00.000Z',
        runIds: (preset as { runIds: string[] }).runIds,
        baselineRunId: (preset as { baselineRunId: string | null }).baselineRunId,
        filters: (preset as { filters?: Record<string, string> }).filters,
      },
    ]);
    deleteBenchmarkComparePresetMock.mockImplementation(async () => []);
  });

  it('filters runs by shortlist fields and updates the visible candidate pool', async () => {
    getBenchmarksMock.mockResolvedValue([
      makeBenchmarkRunSummary('run-alpha', {
        target_host: 'alpha.example.com',
        scenario: 'warm',
        server_region: 'us-east-1',
        network_type: 'loopback',
      }),
      makeBenchmarkRunSummary('run-beta', {
        target_host: 'beta.example.com',
        scenario: 'cold',
        server_region: 'eu-west-1',
        network_type: 'wan',
      }),
    ]);

    renderBenchmarksPage();

    expect(await screen.findByText('Saved Compare Presets')).toBeInTheDocument();
    expect(screen.getAllByText('alpha.example.com').length).toBeGreaterThan(0);
    expect(screen.getAllByText('beta.example.com').length).toBeGreaterThan(0);

    await userEvent.selectOptions(screen.getByLabelText('Scenario'), 'warm');
    await userEvent.selectOptions(screen.getByLabelText('Server region'), 'us-east-1');
    await userEvent.selectOptions(screen.getByLabelText('Network type'), 'loopback');

    await waitFor(() => {
      expect(screen.queryByText('beta.example.com')).not.toBeInTheDocument();
    });
    expect(screen.getAllByText('alpha.example.com').length).toBeGreaterThan(0);
    expect(screen.getByText('showing 1 of 2 loaded runs')).toBeInTheDocument();
  });

  it('saves and reapplies a compare preset with shortlist filters and run selection', async () => {
    getBenchmarksMock.mockResolvedValue([
      makeBenchmarkRunSummary('run-alpha', {
        target_host: 'alpha.example.com',
        scenario: 'warm',
      }),
      makeBenchmarkRunSummary('run-beta', {
        run_id: 'run-beta',
        target_host: 'beta.example.com',
        scenario: 'warm',
      }),
      makeBenchmarkRunSummary('run-gamma', {
        run_id: 'run-gamma',
        target_host: 'gamma.example.com',
        scenario: 'cold',
      }),
    ]);

    renderBenchmarksPage();

    expect(await screen.findByText('Saved Compare Presets')).toBeInTheDocument();

    await userEvent.selectOptions(screen.getByLabelText('Scenario'), 'warm');
    await userEvent.click(screen.getAllByLabelText('Select benchmark run-alpha')[0]);
    await userEvent.click(screen.getAllByLabelText('Select benchmark run-beta')[0]);
    await userEvent.type(screen.getByLabelText('Preset name'), 'Warm compare');
    await userEvent.click(screen.getByRole('button', { name: 'Save current selection' }));

    expect(await screen.findByText('Saved preset Warm compare.')).toBeInTheDocument();
    expect(screen.getByText('Warm compare')).toBeInTheDocument();

    await userEvent.click(screen.getAllByLabelText('Select benchmark run-alpha')[0]);
    await userEvent.click(screen.getAllByLabelText('Select benchmark run-beta')[0]);
    await userEvent.click(screen.getByRole('button', { name: 'Reset filters' }));
    await userEvent.selectOptions(screen.getByLabelText('Scenario'), 'cold');
    await userEvent.click(screen.getByRole('button', { name: 'Apply' }));

    await waitFor(() => {
      expect(screen.getByLabelText('Scenario')).toHaveValue('warm');
    });
    expect(screen.getByText('Loaded preset Warm compare.')).toBeInTheDocument();
    expect(screen.getAllByLabelText('Select benchmark run-alpha')[0]).toBeChecked();
    expect(screen.getAllByLabelText('Select benchmark run-beta')[0]).toBeChecked();
    expect(screen.queryByText('gamma.example.com')).not.toBeInTheDocument();
  });
});
