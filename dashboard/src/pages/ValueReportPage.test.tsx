// ValueReportPage behavior: comparison table sorted best-value-first, the
// missing-cost-SKU banner (rows shown with '—', never dropped), the
// <2-providers empty state, and the as_of disclaimer footer.

import type { ReactNode } from 'react';
import { render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { MemoryRouter } from 'react-router-dom';
import { ValueReportPage } from './ValueReportPage';
import { resetRoleStores, setProjectRole } from '../test/rbac-helpers';
import type { PerfPerCostReport } from '../api/types';

// Recharts' ResponsiveContainer measures a real DOM box — jsdom reports 0×0,
// so render the chart area as a stub; the table is what we assert on.
vi.mock('recharts', async () => {
  const actual = await vi.importActual<Record<string, unknown>>('recharts');
  return {
    ...actual,
    ResponsiveContainer: ({ children }: { children?: ReactNode }) => (
      <div data-testid="chart">{children}</div>
    ),
  };
});

const report: PerfPerCostReport = {
  generated_at: '2026-07-20T12:00:00Z',
  cost_table: {
    as_of: '2026-07-20',
    disclaimer: 'On-demand list prices for comparison only.',
    source: 'shared/cloud-costs.json',
  },
  formulas: {
    latency_cost_index: 'latency_cost_index = p95_ms * hourly_usd (lower is better)',
    mbps_per_dollar_hour: 'mbps_per_dollar_hour = median_throughput_mbps / hourly_usd (higher is better)',
  },
  completed_runs: 12,
  providers_with_data: 3,
  groups: [
    {
      provider: 'azure',
      vm_size: 'Standard_B2s',
      region: 'eastus',
      hourly_usd: 0.0416,
      cost_region: 'eastus',
      cost_source_url: 'https://prices.azure.com/x',
      cost_as_of: '2026-07-20',
      cost_note: null,
      families: [
        {
          family: 'http', run_count: 4, sample_count: 200,
          metric_label: 'latency_ms', median: 42.1, p95_ms: 120,
          value_metric: 'latency_cost_index', value_score: 4.992,
        },
      ],
    },
    {
      provider: 'aws',
      vm_size: 't3.medium',
      region: 'us-east-1',
      hourly_usd: 0.0416,
      cost_region: 'us-east-1',
      cost_source_url: 'https://aws.example/x',
      cost_as_of: '2026-07-17',
      cost_note: null,
      families: [
        {
          family: 'http', run_count: 4, sample_count: 180,
          metric_label: 'latency_ms', median: 30.4, p95_ms: 80,
          value_metric: 'latency_cost_index', value_score: 3.328,
        },
      ],
    },
    {
      provider: 'gcp',
      vm_size: 'weird-size',
      region: 'us-east1',
      hourly_usd: null,
      cost_region: null,
      cost_source_url: null,
      cost_as_of: null,
      cost_note: 'no price row for this SKU in shared/cloud-costs.json — value scores unavailable',
      families: [
        {
          family: 'http', run_count: 1, sample_count: 20,
          metric_label: 'latency_ms', median: 25.0, p95_ms: 60,
          value_metric: 'latency_cost_index', value_score: null,
        },
      ],
    },
  ],
  missing_cost_skus: [{ provider: 'gcp', vm_size: 'weird-size', region: 'us-east1' }],
};

const getPerfPerCostReport = vi.fn((_projectId?: string) => Promise.resolve(report));

vi.mock('../api/client', () => ({
  api: {
    getPerfPerCostReport: (...args: unknown[]) => getPerfPerCostReport(...(args as [string])),
  },
}));

function renderPage() {
  setProjectRole('viewer'); // member-read: a viewer sees the full report
  return render(
    <MemoryRouter initialEntries={['/projects/p-1/reports/value']}>
      <ValueReportPage />
    </MemoryRouter>,
  );
}

describe('ValueReportPage', () => {
  afterEach(() => {
    resetRoleStores();
    vi.clearAllMocks();
  });

  it('renders the comparison table sorted best value first', async () => {
    renderPage();

    await waitFor(() => expect(screen.getByText('t3.medium')).toBeInTheDocument());

    // Latency index ascending: aws (3.328) < azure (4.992); unpriced gcp last.
    const rows = screen.getAllByRole('row').slice(1); // drop header
    expect(rows[0]).toHaveTextContent('aws');
    expect(rows[0]).toHaveTextContent('3.328');
    expect(rows[1]).toHaveTextContent('azure');
    expect(rows[1]).toHaveTextContent('4.992');
    expect(rows[2]).toHaveTextContent('gcp');

    // Header names the formula direction (also appears in the footer text).
    expect(screen.getByText(/p95 × \$\/hr/)).toBeInTheDocument();
    expect(screen.getAllByText(/lower.*is better/).length).toBeGreaterThan(0);
  });

  it('shows unpriced SKUs with a dash and the missing-cost banner, never dropping them', async () => {
    renderPage();

    await waitFor(() => expect(screen.getByText('weird-size')).toBeInTheDocument());

    // Banner counts + names the unpriced group.
    expect(screen.getByText(/1 tester group has no\s+price row/)).toBeInTheDocument();
    expect(screen.getByText(/gcp\/weird-size/)).toBeInTheDocument();

    // The gcp row still renders its perf, with '—' for cost and value.
    const gcpRow = screen.getAllByRole('row').find(r => r.textContent?.includes('gcp'))!;
    expect(gcpRow).toHaveTextContent('25.0ms');
    expect(gcpRow).toHaveTextContent('—');
  });

  it('shows the disclaimer with the cost-table as_of date and the formulas', async () => {
    renderPage();

    await waitFor(() => expect(screen.getByText(/as of\s*2026-07-20/)).toBeInTheDocument());
    expect(screen.getByText(/On-demand list prices for comparison only/)).toBeInTheDocument();
    expect(screen.getByText(/latency_cost_index = p95_ms \* hourly_usd/)).toBeInTheDocument();
    expect(screen.getByText(/mbps_per_dollar_hour = median_throughput_mbps \/ hourly_usd/)).toBeInTheDocument();
  });

  it('shows the comparison empty state when fewer than two providers have data', async () => {
    getPerfPerCostReport.mockResolvedValueOnce({
      ...report,
      providers_with_data: 1,
      groups: [report.groups[0]],
      missing_cost_skus: [],
    });
    renderPage();

    await waitFor(() => expect(
      screen.getByText(/Need testers on at least two providers/)).toBeInTheDocument());
    expect(screen.getByText(/Only azure has completed-run data/)).toBeInTheDocument();
    expect(screen.queryByRole('table')).not.toBeInTheDocument();
  });

  it('shows the no-data empty state when there are no groups at all', async () => {
    getPerfPerCostReport.mockResolvedValueOnce({
      ...report,
      providers_with_data: 0,
      groups: [],
      missing_cost_skus: [],
    });
    renderPage();

    await waitFor(() => expect(
      screen.getByText(/No probe data to price yet/)).toBeInTheDocument());
  });
});
