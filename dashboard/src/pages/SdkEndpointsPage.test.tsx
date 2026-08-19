// SdkEndpointsPage: RBAC mutation gating (operator-write / viewer-read),
// token-masked rendering (the token is NEVER shown; '********' when set), the
// empty state, delete-with-confirm, and the reachability chip (#765: dead
// endpoints must not render identically to live ones).

import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { MemoryRouter } from 'react-router';
import { SdkEndpointsPage } from './SdkEndpointsPage';
import { sdkReachability } from '../lib/sdkReachability';
import { resetRoleStores, setProjectRole, type ProjectRole } from '../test/rbac-helpers';
import type { SdkEndpoint } from '../api/types';

const endpoint: SdkEndpoint = {
  id: 'sdk-1',
  project_id: 'p-1',
  name: 'Checkout API',
  description: 'prod checkout',
  mode: 'sdkprobe',
  url: 'https://api.customer.com',
  route: '/laghound/echo',
  token_set: true,
  token: '********',
  max_duration_secs: 900,
  created_by: 'u-1',
  created_at: '2026-07-20T12:00:00Z',
  updated_at: '2026-07-20T12:00:00Z',
  last_run_at: '2026-08-17T09:00:00Z',
  last_run_status: 'completed',
  last_run_success_count: 10,
  last_run_failure_count: 0,
};

const listSdkEndpoints = vi.fn(() => Promise.resolve([endpoint]));
const deleteSdkEndpoint = vi.fn(() => Promise.resolve(undefined));

vi.mock('../api/client', () => ({
  errorMessage: (e: unknown) => (e instanceof Error ? e.message : String(e)),
  api: {
    listSdkEndpoints: (...a: unknown[]) => listSdkEndpoints(...(a as [])),
    deleteSdkEndpoint: (...a: unknown[]) => deleteSdkEndpoint(...(a as [])),
  },
}));

const addToast = vi.fn();
vi.mock('../hooks/useToast', () => ({ useToast: () => addToast }));

async function renderPage(role: ProjectRole) {
  setProjectRole(role);
  const utils = render(
    <MemoryRouter initialEntries={['/projects/p-1/sdk-endpoints']}>
      <SdkEndpointsPage />
    </MemoryRouter>,
  );
  await waitFor(() => expect(screen.getByText('Checkout API')).toBeInTheDocument());
  return utils;
}

describe('SdkEndpointsPage', () => {
  afterEach(() => {
    resetRoleStores();
    vi.clearAllMocks();
  });

  it('renders the token masked, never the real token', async () => {
    await renderPage('viewer');
    // The masked token is shown; no raw secret ever appears.
    expect(screen.getByText('********')).toBeInTheDocument();
    expect(screen.getByText('https://api.customer.com')).toBeInTheDocument();
  });

  it('viewer: read-only — no create/delete affordances', async () => {
    await renderPage('viewer');
    expect(screen.queryByRole('button', { name: '+ SDK endpoint' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /Delete Checkout API/ })).not.toBeInTheDocument();
  });

  it('operator: has create + delete affordances', async () => {
    await renderPage('operator');
    expect(screen.getByRole('button', { name: '+ SDK endpoint' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Delete Checkout API' })).toBeInTheDocument();
  });

  it('admin: same mutation surface as operator', async () => {
    await renderPage('admin');
    expect(screen.getByRole('button', { name: '+ SDK endpoint' })).toBeInTheDocument();
  });

  it('operator: delete asks for confirmation before calling the API', async () => {
    const user = userEvent.setup();
    await renderPage('operator');
    await user.click(screen.getByRole('button', { name: 'Delete Checkout API' }));

    const dialog = screen.getByRole('alertdialog');
    expect(within(dialog).getByText('Delete SDK endpoint')).toBeInTheDocument();
    expect(within(dialog).getByText(/past probe runs, and report history/)).toBeInTheDocument();
    expect(deleteSdkEndpoint).not.toHaveBeenCalled();

    await user.click(within(dialog).getByRole('button', { name: 'Delete' }));
    await waitFor(() => expect(deleteSdkEndpoint).toHaveBeenCalledWith('p-1', 'sdk-1'));
  });

  it('shows the empty state with a create CTA for operators when there are no endpoints', async () => {
    listSdkEndpoints.mockResolvedValueOnce([]);
    setProjectRole('operator');
    render(
      <MemoryRouter initialEntries={['/projects/p-1/sdk-endpoints']}>
        <SdkEndpointsPage />
      </MemoryRouter>,
    );
    await waitFor(() => expect(screen.getByText('No SDK endpoints yet')).toBeInTheDocument());
    expect(screen.getByRole('button', { name: /Register your first SDK endpoint/i })).toBeInTheDocument();
  });

  it('viewer sees the empty state without a create CTA', async () => {
    listSdkEndpoints.mockResolvedValueOnce([]);
    setProjectRole('viewer');
    render(
      <MemoryRouter initialEntries={['/projects/p-1/sdk-endpoints']}>
        <SdkEndpointsPage />
      </MemoryRouter>,
    );
    await waitFor(() => expect(screen.getByText('No SDK endpoints yet')).toBeInTheDocument());
    expect(screen.queryByRole('button', { name: /Register your first SDK endpoint/i })).not.toBeInTheDocument();
  });

  // ── Reachability chip (#765) ──────────────────────────────────────────

  it('renders a reachable chip for a green last run', async () => {
    await renderPage('viewer');
    expect(screen.getByText('reachable')).toBeInTheDocument();
  });

  it('renders an unreachable chip when the last probe failed', async () => {
    listSdkEndpoints.mockResolvedValueOnce([
      { ...endpoint, last_run_status: 'completed', last_run_success_count: 0, last_run_failure_count: 10 },
    ]);
    await renderPage('viewer');
    expect(screen.getByText('unreachable')).toBeInTheDocument();
  });

  it('renders never probed when there is no run', async () => {
    listSdkEndpoints.mockResolvedValueOnce([
      { ...endpoint, last_run_at: null, last_run_status: null, last_run_success_count: null, last_run_failure_count: null },
    ]);
    await renderPage('viewer');
    expect(screen.getByText('never probed')).toBeInTheDocument();
  });
});

describe('sdkReachability', () => {
  const of = (status: string | null, ok: number | null, fail: number | null) =>
    sdkReachability({ last_run_status: status, last_run_success_count: ok, last_run_failure_count: fail });

  it('maps run outcomes to verdicts', () => {
    expect(of(null, null, null)).toBe('none');
    expect(of('queued', 0, 0)).toBe('probing');
    expect(of('provisioning', 0, 0)).toBe('probing');
    expect(of('running', 0, 0)).toBe('probing');
    expect(of('completed', 10, 0)).toBe('reachable');
    expect(of('completed', 9, 1)).toBe('partial');
    // Completed-all-fail is the dead-endpoint shape from the E2E pass
    // (connect timeout on every attempt still "completes" the run).
    expect(of('completed', 0, 10)).toBe('unreachable');
    expect(of('failed', 0, 0)).toBe('unreachable');
    expect(of('cancelled', 0, 0)).toBe('unreachable');
    // Completed with no attempts recorded: no verdict, not a false green.
    expect(of('completed', 0, 0)).toBe('none');
  });
});
