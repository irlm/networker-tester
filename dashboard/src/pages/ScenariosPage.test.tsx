import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { MemoryRouter } from 'react-router';
import type { ScenarioReadinessResponse } from '../features/scenarios/api';
import { ScenariosPage } from './ScenariosPage';

const mocks = vi.hoisted(() => ({
  readiness: vi.fn(),
  runs: vi.fn(),
  toast: vi.fn(),
}));

vi.mock('../hooks/useProject', () => ({
  useProject: () => ({ projectId: 'p-1', isOperator: true }),
}));
vi.mock('../features/scenarios/queries', () => ({
  useScenarioReadinessQuery: () => mocks.readiness(),
}));
vi.mock('../features/runs/queries', () => ({
  useTestRunsQuery: () => mocks.runs(),
}));
vi.mock('../hooks/useToast', () => ({ useToast: () => mocks.toast }));

const ready: ScenarioReadinessResponse = {
  runners: { data: [{ tester_id: 't-1', name: 'runner', power_state: 'running', agent_status: 'online' } as never], error: null },
  deployments: { data: [{ deployment_id: 'd-1', name: 'target', status: 'completed', endpoint_ips: ['192.0.2.1'] } as never], error: null },
  cloudAccounts: { data: [{ account_id: 'c-1', name: 'cloud', provider: 'aws', status: 'active' } as never], error: null },
};

function renderPage() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  const tree = (
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={['/projects/p-1/scenarios']}>
        <ScenariosPage />
      </MemoryRouter>
    </QueryClientProvider>
  );
  const result = render(tree);
  return { ...result, rerenderPage: () => result.rerender(tree) };
}

beforeEach(() => {
  localStorage.clear();
  mocks.readiness.mockReturnValue({
    data: ready,
    dataUpdatedAt: new Date('2026-08-18T12:00:00Z').getTime(),
    isFetching: false,
    refetch: vi.fn(),
  });
  mocks.runs.mockReturnValue({ data: [], isPending: false, isError: false, refetch: vi.fn() });
});

afterEach(() => {
  vi.clearAllMocks();
});

describe('ScenariosPage triage console', () => {
  it('switches intent and recommendation with keyboard-only navigation', async () => {
    const user = userEvent.setup();
    renderPage();

    expect(screen.getByRole('heading', { name: 'Quick latency & TLS check' })).toBeInTheDocument();
    await user.keyboard('3');

    expect(screen.getByRole('tab', { name: /Deployed endpoint/ })).toHaveAttribute('aria-selected', 'true');
    expect(screen.getByRole('heading', { name: 'Throughput to your endpoint' })).toBeInTheDocument();
  });

  it('persists an off switch for character shortcuts', async () => {
    const user = userEvent.setup();
    renderPage();

    await user.click(screen.getByRole('button', { name: /Scenario keyboard shortcuts enabled/ }));
    expect(localStorage.getItem('scenario-keys-enabled')).toBe('false');
    await user.keyboard('4');

    expect(screen.getByRole('tab', { name: /URL symptoms/ })).toHaveAttribute('aria-selected', 'true');
  });

  it('shows readiness details and exact repair actions', async () => {
    const user = userEvent.setup();
    mocks.readiness.mockReturnValue({
      data: { ...ready, deployments: { data: [], error: null } },
      dataUpdatedAt: Date.now(),
      isFetching: false,
      refetch: vi.fn(),
    });
    renderPage();

    await user.click(screen.getByRole('tab', { name: /Deployed endpoint/ }));
    const recommended = screen.getByRole('heading', { name: 'Throughput to your endpoint' }).closest('article');
    expect(within(recommended!).getByRole('link', { name: /Deploy endpoint/ })).toHaveAttribute('href', '/projects/p-1/vms');

    await user.click(screen.getByRole('button', { name: 'View status' }));
    expect(screen.getAllByRole('link', { name: /Deploy endpoint/ }).length).toBeGreaterThan(0);
  });

  it('discloses methodology without leaving the launcher', async () => {
    const user = userEvent.setup();
    renderPage();

    await user.keyboard('m');
    expect(screen.getByText(/Uses the existing tested builder/)).toBeInTheDocument();
  });

  it('keeps an id-keyed keyboard selection stable across readiness polls', async () => {
    const user = userEvent.setup();
    let currentReadiness = ready;
    mocks.readiness.mockImplementation(() => ({
      data: currentReadiness,
      dataUpdatedAt: Date.now(),
      isFetching: false,
      refetch: vi.fn(),
    }));
    const { rerenderPage } = renderPage();

    await user.keyboard('j');
    const selected = screen.getByRole('heading', { name: 'Protocol & handshake deep-dive' }).closest('article');
    expect(selected).toHaveAttribute('data-selected', 'true');

    currentReadiness = { ...ready, runners: { data: [], error: null } };
    rerenderPage();

    expect(screen.getByRole('heading', { name: 'Protocol & handshake deep-dive' }).closest('article'))
      .toHaveAttribute('data-selected', 'true');
    expect(screen.getByRole('heading', { name: 'Quick latency & TLS check' })).toBeInTheDocument();
  });

  it('does not offer one-click rerun for a provisioning benchmark', () => {
    mocks.runs.mockReturnValue({
      data: [{
        id: 'run-1', test_config_id: 'config-1', project_id: 'p-1', status: 'completed',
        test_kind: 'benchmark', config_name: 'Runtime matrix', created_at: new Date().toISOString(),
        success_count: 12, failure_count: 0,
      }],
      isPending: false,
      isError: false,
      refetch: vi.fn(),
    });
    renderPage();

    expect(screen.queryByRole('button', { name: 'Run with last configuration' })).not.toBeInTheDocument();
    expect(screen.getByRole('link', { name: /Open result/ })).toHaveAttribute('href', '/projects/p-1/runs/run-1');
    expect(screen.getByText(/fresh testbed review/)).toBeInTheDocument();
  });

  it('fails closed when a recent run omits its test kind', () => {
    mocks.runs.mockReturnValue({
      data: [{
        id: 'run-2', test_config_id: 'config-2', project_id: 'p-1', status: 'completed',
        config_name: 'Partial projection', created_at: new Date().toISOString(),
        success_count: 1, failure_count: 0,
      }],
      isPending: false,
      isError: false,
      refetch: vi.fn(),
    });
    renderPage();

    expect(screen.queryByRole('button', { name: 'Run with last configuration' })).not.toBeInTheDocument();
    expect(screen.getByRole('link', { name: /Open result/ })).toHaveAttribute('href', '/projects/p-1/runs/run-2');
  });
});
