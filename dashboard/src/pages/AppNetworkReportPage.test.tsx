// AppNetworkReportPage: the headline verdict + main-issue string for each
// verdict (server_bound / network_bound / balanced / no_data), the split bar,
// the per-endpoint table with the anomaly warning, the empty state, and the
// verbatim formula disclaimer. member-read (a viewer sees the whole report).

import { render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { MemoryRouter } from 'react-router-dom';
import { AppNetworkReportPage } from './AppNetworkReportPage';
import { resetRoleStores, setProjectRole } from '../test/rbac-helpers';
import type { AppNetworkReport, AppNetworkVerdict } from '../api/types';

function makeReport(overrides: Partial<AppNetworkReport> = {}): AppNetworkReport {
  return {
    generated_at: '2026-07-21T12:00:00Z',
    formulas: {
      server_ms: 'server_ms = ServerTimingResult.TotalServerMs',
      network_ms: 'network_ms = max(0, wall_ms - server_ms)',
      split: 'a side is dominant when its median >= 0.6 of median wall',
      split_anomaly: 'split_anomaly = server_ms > wall_ms',
    },
    mode: 'sdkprobe',
    attempt_count: 100,
    split_anomaly_count: 0,
    overall_verdict: 'server_bound',
    overall_main_issue:
      'Server processing dominates: ~80ms of ~100ms — investigate your application, not the network.',
    overall_median_server_ms: 80,
    overall_median_network_ms: 20,
    overall_median_wall_ms: 100,
    overall_server_ratio: 0.8,
    groups: [
      {
        config_id: 'g-1',
        config_name: 'Checkout API',
        run_count: 5,
        attempt_count: 100,
        split_anomaly_count: 0,
        median_server_ms: 80,
        p95_server_ms: 120,
        median_network_ms: 20,
        p95_network_ms: 35,
        median_wall_ms: 100,
        server_ratio: 0.8,
        verdict: 'server_bound',
        main_issue: 'Server processing dominates: ~80ms of ~100ms.',
      },
    ],
    ...overrides,
  };
}

const getAppNetworkReport = vi.fn(() => Promise.resolve(makeReport()));

vi.mock('../api/client', () => ({
  api: {
    getAppNetworkReport: (...a: unknown[]) => getAppNetworkReport(...(a as [string])),
  },
}));

function renderPage() {
  setProjectRole('viewer'); // member-read
  return render(
    <MemoryRouter initialEntries={['/projects/p-1/reports/app-network']}>
      <AppNetworkReportPage />
    </MemoryRouter>,
  );
}

describe('AppNetworkReportPage', () => {
  afterEach(() => {
    resetRoleStores();
    vi.clearAllMocks();
  });

  it('renders the server-bound headline with the overall main-issue string', async () => {
    renderPage();
    await waitFor(() =>
      expect(screen.getByText(/Server processing dominates: ~80ms of ~100ms/)).toBeInTheDocument(),
    );
    // "Server-bound" appears twice: the headline label and the table badge.
    expect(screen.getAllByText('Server-bound').length).toBeGreaterThanOrEqual(1);
    // Split bar renders as an accessible image with net/server label.
    expect(screen.getAllByRole('img', { name: /Latency split/ }).length).toBeGreaterThan(0);
    // Server ratio shown as a percentage.
    expect(screen.getByText('server ratio 80%')).toBeInTheDocument();
  });

  it.each<[AppNetworkVerdict, string, string]>([
    ['server_bound', 'Server-bound', 'Server processing dominates: ~80ms of ~100ms — investigate your application, not the network.'],
    ['network_bound', 'Network-bound', 'Network transit dominates: ~70ms of ~100ms — investigate connectivity/routing, not your application.'],
    ['balanced', 'Balanced', 'Balanced: ~50ms server vs ~50ms network of ~100ms — no single dominant cost.'],
  ])('renders the %s verdict headline', async (verdict, label, issue) => {
    getAppNetworkReport.mockResolvedValueOnce(
      makeReport({ overall_verdict: verdict, overall_main_issue: issue }),
    );
    renderPage();
    // The label may appear on both the headline and a table badge — assert on
    // the unique main-issue string, and that the label renders at least once.
    await waitFor(() => expect(screen.getByText(issue)).toBeInTheDocument());
    expect(screen.getAllByText(label).length).toBeGreaterThanOrEqual(1);
  });

  it('surfaces the split-anomaly warning when anomalies are present', async () => {
    getAppNetworkReport.mockResolvedValueOnce(
      makeReport({
        split_anomaly_count: 3,
        groups: [{ ...makeReport().groups[0], split_anomaly_count: 3 }],
      }),
    );
    renderPage();
    await waitFor(() => expect(screen.getByText(/3 split anomalies/)).toBeInTheDocument());
    // Per-endpoint table also flags it.
    const table = screen.getByRole('table');
    expect(within(table).getByText(/⚠ 3/)).toBeInTheDocument();
  });

  it('renders the per-endpoint table with median/p95 server & network', async () => {
    renderPage();
    await waitFor(() => expect(screen.getByRole('table')).toBeInTheDocument());
    const table = screen.getByRole('table');
    expect(within(table).getByText('Checkout API')).toBeInTheDocument();
    // p95s appear in both the numeric cell and the split-bar caption — assert
    // presence (>=1), not uniqueness.
    expect(within(table).getAllByText(/120\.0ms/).length).toBeGreaterThanOrEqual(1); // p95 server
    expect(within(table).getAllByText(/35\.0ms/).length).toBeGreaterThanOrEqual(1); // p95 network
  });

  it('renders the formula disclaimer verbatim from the response', async () => {
    renderPage();
    await waitFor(() =>
      expect(screen.getByText('server_ms = ServerTimingResult.TotalServerMs')).toBeInTheDocument(),
    );
    expect(screen.getByText('network_ms = max(0, wall_ms - server_ms)')).toBeInTheDocument();
    expect(screen.getByText('split_anomaly = server_ms > wall_ms')).toBeInTheDocument();
  });

  it('shows the empty state linking to create an SDK endpoint on no_data', async () => {
    getAppNetworkReport.mockResolvedValueOnce(
      makeReport({ overall_verdict: 'no_data', attempt_count: 0, groups: [] }),
    );
    renderPage();
    await waitFor(() => expect(screen.getByText('No sdkprobe data yet')).toBeInTheDocument());
    const link = screen.getByRole('link', { name: /Create an SDK endpoint/i });
    expect(link).toHaveAttribute('href', '/projects/p-1/sdk-endpoints');
    expect(screen.queryByRole('table')).not.toBeInTheDocument();
  });
});
