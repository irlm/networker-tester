import { MemoryRouter } from 'react-router';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';

const queryMocks = vi.hoisted(() => ({
  logs: vi.fn(),
  stats: vi.fn(),
  refetchLogs: vi.fn(),
  refetchStats: vi.fn(),
}));

vi.mock('../features/perf-logs/queries', () => ({
  usePerfLogsQuery: queryMocks.logs,
  usePerfLogStatsQuery: queryMocks.stats,
}));

import { PerfLogPage } from './PerfLogPage';

const stats = {
  api_count: 12,
  render_count: 8,
  avg_total_ms: 40,
  avg_server_ms: 25,
  avg_render_ms: 10,
  p95_total_ms: 80,
  p95_render_ms: 14,
  slow_api_count: 1,
  janky_render_count: 0,
};

beforeEach(() => {
  queryMocks.logs.mockReset();
  queryMocks.stats.mockReset();
  queryMocks.refetchLogs.mockReset();
  queryMocks.refetchStats.mockReset();
  queryMocks.logs.mockReturnValue({
    data: [],
    error: null,
    isPending: false,
    isError: false,
    dataUpdatedAt: Date.now(),
    refetch: queryMocks.refetchLogs,
  });
  queryMocks.stats.mockReturnValue({
    data: stats,
    error: null,
    isPending: false,
    isError: false,
    dataUpdatedAt: Date.now(),
    refetch: queryMocks.refetchStats,
  });
});

function renderPage(entry = '/admin/perf-log') {
  return render(
    <MemoryRouter initialEntries={[entry]}>
      <PerfLogPage />
    </MemoryRouter>,
  );
}

describe('PerfLogPage time filters', () => {
  it('starts at five minutes and passes that window to list and stats queries', () => {
    renderPage();

    expect(screen.getByRole('combobox', { name: 'Time range' })).toHaveValue('5m');
    expect(queryMocks.logs).toHaveBeenLastCalledWith(
      expect.objectContaining({ windowMs: 300_000, limit: 200 }),
      true,
    );
    expect(queryMocks.stats).toHaveBeenLastCalledWith(300_000, false);
    expect(screen.getByText(/No performance logs in last 5 minutes/i)).toBeInTheDocument();
  });

  it('reads a time range from the URL and updates the server query when changed', async () => {
    const user = userEvent.setup();
    renderPage('/admin/perf-log?range=24h');

    const range = screen.getByRole('combobox', { name: 'Time range' });
    expect(range).toHaveValue('24h');
    expect(queryMocks.logs).toHaveBeenLastCalledWith(
      expect.objectContaining({ windowMs: 24 * 60 * 60_000 }),
      true,
    );

    await user.selectOptions(range, '1h');

    await waitFor(() => expect(queryMocks.logs).toHaveBeenLastCalledWith(
      expect.objectContaining({ windowMs: 60 * 60_000 }),
      true,
    ));
    expect(queryMocks.stats).toHaveBeenLastCalledWith(60 * 60_000, false);
  });

  it('keeps kind and path filters on the log view and exposes an accessible stats tab', async () => {
    const user = userEvent.setup();
    renderPage();

    await user.selectOptions(screen.getByRole('combobox', { name: 'Log kind' }), 'api');
    await user.type(screen.getByRole('searchbox', { name: 'Filter logs by path' }), '/health');

    await waitFor(() => expect(queryMocks.logs).toHaveBeenLastCalledWith(
      expect.objectContaining({ kind: 'api', path: '/health', windowMs: 300_000 }),
      true,
    ));

    await user.click(screen.getByRole('tab', { name: 'Stats' }));
    expect(screen.getByRole('tab', { name: 'Stats' })).toHaveAttribute('aria-selected', 'true');
    expect(queryMocks.logs).toHaveBeenLastCalledWith(expect.anything(), false);
    expect(queryMocks.stats).toHaveBeenLastCalledWith(300_000, true);
    expect(screen.queryByRole('combobox', { name: 'Log kind' })).not.toBeInTheDocument();
    expect(screen.getByText('API Requests')).toBeInTheDocument();
  });

  it('stops both polling intervals when paused before switching views', async () => {
    const user = userEvent.setup();
    renderPage();

    await user.click(screen.getByRole('button', { name: 'LIVE' }));
    expect(queryMocks.logs).toHaveBeenLastCalledWith(expect.anything(), false);
    expect(queryMocks.stats).toHaveBeenLastCalledWith(300_000, false);

    await user.click(screen.getByRole('tab', { name: 'Stats' }));
    expect(queryMocks.stats).toHaveBeenLastCalledWith(300_000, false);
  });

  it('supports arrow, Home, and End keyboard navigation between tabs', async () => {
    const user = userEvent.setup();
    renderPage();

    const logsTab = screen.getByRole('tab', { name: /Logs/ });
    logsTab.focus();
    await user.keyboard('{ArrowRight}');

    const statsTab = screen.getByRole('tab', { name: 'Stats' });
    expect(statsTab).toHaveFocus();
    expect(statsTab).toHaveAttribute('aria-selected', 'true');
    expect(screen.getByRole('tabpanel')).toHaveAttribute('aria-labelledby', 'performance-stats-tab');

    await user.keyboard('{Home}');
    expect(logsTab).toHaveFocus();
    expect(logsTab).toHaveAttribute('aria-selected', 'true');
  });
});

describe('PerfLogPage statistics emphasis', () => {
  /** One API row, as the list query returns them. */
  const apiLog = (path: string, totalMs: number, i: number) => ({
    id: `${path}-${i}`,
    logged_at: new Date().toISOString(),
    kind: 'api' as const,
    method: 'GET',
    path,
    status: 200,
    total_ms: totalMs,
    server_ms: 5,
    network_ms: totalMs - 5,
    source: 'poll',
    component: null,
    trigger: null,
    render_ms: null,
    item_count: null,
  });

  it('leads with p95 and demotes the average to the sub-line', async () => {
    renderPage();
    await userEvent.click(screen.getByRole('tab', { name: 'Stats' }));

    // p95 is the headline: an average cannot show a latency problem, because
    // the many fast polls drag it away from the tail people actually feel.
    expect(await screen.findByText('API p95')).toBeInTheDocument();
    expect(screen.getByText('Render p95')).toBeInTheDocument();
    expect(screen.getByText('avg 40.0ms')).toBeInTheDocument();
    expect(screen.getByText('avg 10.0ms')).toBeInTheDocument();
  });

  it('reports the slow share, not just the slow count', async () => {
    renderPage();
    await userEvent.click(screen.getByRole('tab', { name: 'Stats' }));
    // 1 of 12 API calls over 200 ms.
    expect(await screen.findByText(/1 slow >200ms \(8\.3%\)/)).toBeInTheDocument();
  });

  it('ranks endpoints by total time contributed, not by average', async () => {
    // /reports is slower per call but runs twice; /version is quick and polled
    // ten times and therefore costs more wall clock. Ranking by average puts
    // the wrong one first, which is the bug this ordering fixes.
    queryMocks.logs.mockReturnValue({
      data: [
        ...Array.from({ length: 10 }, (_, i) => apiLog('/api/version', 40, i)),   // 400 ms total
        ...Array.from({ length: 2 }, (_, i) => apiLog('/api/reports', 150, i)),   // 300 ms total
      ],
      error: null,
      isPending: false,
      isError: false,
      dataUpdatedAt: Date.now(),
      refetch: queryMocks.refetchLogs,
    });

    renderPage();
    await userEvent.click(screen.getByRole('tab', { name: 'Stats' }));

    const heading = await screen.findByText(/Where the time goes/i);
    const table = heading.parentElement!.querySelector('table')!;
    const firstPath = table.querySelectorAll('tbody tr td')[0];
    expect(firstPath).toHaveTextContent('/api/version');

    // And the total that justifies the ordering is on screen, not implied.
    expect(table).toHaveTextContent('400.0ms');
  });
});
