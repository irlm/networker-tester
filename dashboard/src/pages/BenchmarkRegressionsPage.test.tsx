import { render, screen } from '@testing-library/react';
import { MemoryRouter } from 'react-router';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { BenchmarkRegressionSummary, BenchmarkRegressionWithConfig } from '../api/types';

// #810: the page must distinguish (a) "no baselines exist yet — detection has
// never compared anything" from (b) "N comparisons ran · 0 regressions", and
// keep working (generic copy) when the summary endpoint fails.

const mocks = vi.hoisted(() => ({
  list: vi.fn(),
  summary: vi.fn(),
}));

vi.mock('../api/client', () => ({
  api: {
    listBenchmarkRegressions: mocks.list,
    getBenchmarkRegressionSummary: mocks.summary,
  },
}));
vi.mock('../hooks/useProject', () => ({
  useProject: () => ({ projectId: 'p-1' }),
}));

import { BenchmarkRegressionsPage } from './BenchmarkRegressionsPage';

function summary(overrides: Partial<BenchmarkRegressionSummary> = {}): BenchmarkRegressionSummary {
  return {
    comparable_configs: 0,
    pinned_baseline_configs: 0,
    runs_compared: 0,
    last_comparison_at: null,
    total_regressions: 0,
    last_regression_at: null,
    semantics: 'derived from run history',
    ...overrides,
  };
}

function regression(): BenchmarkRegressionWithConfig {
  return {
    regression_id: 'reg-1',
    config_id: 'cfg-1',
    config_name: 'api-bench-eastus',
    run_id: 'run-2',
    baseline_run_id: 'run-1',
    case_id: 'http1-1024',
    metric: 'p50_latency_ms',
    metric_unit: 'ms',
    baseline_value: 10,
    current_value: 20,
    delta_percent: 100,
    severity: 'critical',
    detected_at: new Date().toISOString(),
  };
}

beforeEach(() => {
  mocks.list.mockReset();
  mocks.summary.mockReset();
});

function renderPage() {
  return render(
    <MemoryRouter>
      <BenchmarkRegressionsPage />
    </MemoryRouter>,
  );
}

describe('BenchmarkRegressionsPage comparison-activity states (#810)', () => {
  it('says detection has never compared anything when no baselines exist', async () => {
    mocks.list.mockResolvedValue([]);
    mocks.summary.mockResolvedValue(summary());
    renderPage();

    expect(await screen.findByText(
      /No baselines available yet — detection has never compared anything/,
    )).toBeInTheDocument();
    // The fix pathway is spelled out: schedules (config reuse) or a pinned baseline.
    expect(screen.getByRole('link', { name: /schedule a benchmark config/i }))
      .toHaveAttribute('href', '/projects/p-1/schedules');
    expect(screen.getByText(/pin it as baseline/)).toBeInTheDocument();
    // No activity strip when nothing was ever compared.
    expect(screen.queryByText('Runs compared')).not.toBeInTheDocument();
  });

  it('shows comparisons ran with zero regressions as a healthy state', async () => {
    mocks.list.mockResolvedValue([]);
    mocks.summary.mockResolvedValue(summary({
      comparable_configs: 3,
      runs_compared: 12,
      last_comparison_at: new Date(Date.now() - 60_000).toISOString(),
    }));
    renderPage();

    expect(await screen.findByText(
      /No regressions detected — 12 runs compared against baselines \(last: .+\)/,
    )).toBeInTheDocument();
    // Activity strip carries the pulse numbers.
    expect(screen.getByText('Runs compared')).toBeInTheDocument();
    expect(screen.getByText('12')).toBeInTheDocument();
    expect(screen.getByText('Comparable configs')).toBeInTheDocument();
    expect(screen.getByText('3')).toBeInTheDocument();
    // Never-compared copy must NOT appear.
    expect(screen.queryByText(/No baselines available yet/)).not.toBeInTheDocument();
  });

  it('falls back to the generic empty state when the summary endpoint fails', async () => {
    mocks.list.mockResolvedValue([]);
    mocks.summary.mockRejectedValue(new Error('boom'));
    renderPage();

    expect(await screen.findByText('No regressions detected')).toBeInTheDocument();
    expect(screen.queryByText(/No baselines available yet/)).not.toBeInTheDocument();
  });

  it('renders the regressions table plus the activity strip when breaches exist', async () => {
    mocks.list.mockResolvedValue([regression()]);
    mocks.summary.mockResolvedValue(summary({
      comparable_configs: 1,
      runs_compared: 2,
      total_regressions: 1,
      last_comparison_at: new Date().toISOString(),
      last_regression_at: new Date().toISOString(),
    }));
    renderPage();

    expect(await screen.findByText('api-bench-eastus')).toBeInTheDocument();
    expect(screen.getByText('critical')).toBeInTheDocument();
    expect(screen.getByText('Runs compared')).toBeInTheDocument();
  });
});
