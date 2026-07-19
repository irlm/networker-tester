// AlertsPage behavior: history rendering (newest-first paginated event list
// with rule context + delivery status) and the 409 guard when deleting a
// channel that rules still reference.

import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { MemoryRouter } from 'react-router-dom';
import { AlertsPage } from './AlertsPage';
import { resetRoleStores, setProjectRole } from '../test/rbac-helpers';
import { useToastStore } from '../hooks/useToast';
import type { AlertChannel, AlertEvent, AlertRule } from '../api/types';

const rule: AlertRule = {
  rule_id: 'r-1',
  project_id: 'p-1',
  test_config_id: 'cfg-1',
  metric: 'p95_ms',
  comparator: 'gt',
  threshold: 500,
  window_runs: 3,
  enabled: true,
  channel_id: 'ch-1',
  created_by: null,
  created_at: '2026-07-18T00:00:00Z',
};

const channel: AlertChannel = {
  channel_id: 'ch-1',
  project_id: 'p-1',
  kind: 'webhook',
  name: 'ops hook',
  config: { url: 'https://hooks.example.com/x' },
  enabled: true,
  created_at: '2026-07-18T00:00:00Z',
};

// Newest first, as the API returns them.
const events: AlertEvent[] = [
  {
    event_id: 'ev-2',
    rule_id: 'r-1',
    run_id: 'run-2',
    fired_at: '2026-07-18T04:00:00Z',
    state: 'resolved',
    value: 320.5,
    message: 'p95_ms recovered below 500',
    delivery_status: 'delivered',
    metric: 'p95_ms',
    comparator: 'gt',
    threshold: 500,
    test_config_id: 'cfg-1',
    channel_id: 'ch-1',
  },
  {
    event_id: 'ev-1',
    rule_id: 'r-1',
    run_id: 'run-1',
    fired_at: '2026-07-18T03:00:00Z',
    state: 'firing',
    value: 812.5,
    message: 'p95_ms 812.5 > 500 for 3 consecutive run(s)',
    delivery_status: 'failed: http 500',
    metric: 'p95_ms',
    comparator: 'gt',
    threshold: 500,
    test_config_id: 'cfg-1',
    channel_id: 'ch-1',
  },
];

vi.mock('../api/client', () => {
  class MockApiError extends Error {
    readonly status: number;
    readonly body: string | null;
    constructor(status: number, message: string, body: string | null = null) {
      super(message);
      this.status = status;
      this.body = body;
    }
  }
  return {
    ApiError: MockApiError,
    api: {
      listAlertRules: vi.fn(() => Promise.resolve([rule])),
      listAlertChannels: vi.fn(() => Promise.resolve([channel])),
      listTestConfigs: vi.fn(() =>
        Promise.resolve([
          {
            id: 'cfg-1',
            project_id: 'p-1',
            name: 'prod-api-latency',
            endpoint_kind: 'network',
            modes: ['http1'],
            has_methodology: false,
            created_at: '2026-07-18T00:00:00Z',
            updated_at: '2026-07-18T00:00:00Z',
          },
        ]),
      ),
      listAlertEvents: vi.fn(() => Promise.resolve(events)),
      deleteAlertChannel: vi.fn(),
    },
  };
});

import { api, ApiError } from '../api/client';

function renderPage(tab: 'history' | 'channels') {
  setProjectRole('operator');
  return render(
    <MemoryRouter initialEntries={[`/projects/p-1/alerts?tab=${tab}`]}>
      <AlertsPage />
    </MemoryRouter>,
  );
}

describe('AlertsPage history', () => {
  afterEach(() => {
    resetRoleStores();
    useToastStore.setState({ toasts: [] });
    vi.clearAllMocks();
  });

  it('renders events newest-first with rule context, state, and delivery status', async () => {
    renderPage('history');

    await waitFor(() => expect(screen.getByText('resolved')).toBeInTheDocument());

    const rows = screen.getAllByRole('row').slice(1); // skip header
    expect(rows).toHaveLength(2);

    // Newest (resolved) first, exactly as the API ordered them.
    expect(within(rows[0]).getByText('resolved')).toBeInTheDocument();
    expect(within(rows[0]).getByText('320.5ms')).toBeInTheDocument();
    expect(within(rows[0]).getByText('delivered')).toBeInTheDocument();

    expect(within(rows[1]).getByText('firing')).toBeInTheDocument();
    expect(within(rows[1]).getByText('812.5ms')).toBeInTheDocument();
    expect(within(rows[1]).getByText('failed: http 500')).toBeInTheDocument();

    // Rule context rendered standalone on each row.
    expect(within(rows[1]).getByText('p95_ms > 500ms')).toBeInTheDocument();
    expect(within(rows[1]).getByText('prod-api-latency')).toBeInTheDocument();

    // First page requested from offset 0, newest first, page-sized.
    expect(api.listAlertEvents).toHaveBeenCalledWith('p-1', { limit: 50, offset: 0 });
  });

  it('filters history by rule via ?rule_id=', async () => {
    const user = userEvent.setup();
    renderPage('history');
    await waitFor(() => expect(screen.getByText('resolved')).toBeInTheDocument());

    await user.selectOptions(screen.getByLabelText('Filter history by rule'), 'r-1');
    await waitFor(() =>
      expect(api.listAlertEvents).toHaveBeenCalledWith('p-1', { limit: 50, offset: 0, rule_id: 'r-1' }),
    );
  });

  it('surfaces the 409 conflict when deleting a channel that rules reference', async () => {
    vi.mocked(api.deleteAlertChannel).mockRejectedValue(
      new ApiError(409, 'conflict', '{"error":"channel is referenced by alert rules — delete or repoint them first"}'),
    );
    const user = userEvent.setup();
    renderPage('channels');
    await waitFor(() => expect(screen.getByText('ops hook')).toBeInTheDocument());

    const del = screen.getByRole('button', { name: 'Delete channel' });
    await user.click(del); // arm two-step confirm
    await user.click(screen.getByRole('button', { name: 'Confirm delete channel' }));

    await waitFor(() => expect(api.deleteAlertChannel).toHaveBeenCalledWith('ch-1'));
    await waitFor(() =>
      expect(useToastStore.getState().toasts.some((t) => t.type === 'error' && /referenced by alert rules/.test(t.message))).toBe(true),
    );
  });
});
