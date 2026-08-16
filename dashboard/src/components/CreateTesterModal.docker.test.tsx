import { render, screen, waitFor, fireEvent } from '@testing-library/react';
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { MemoryRouter } from 'react-router';
import type { ReactNode } from 'react';
import { CreateTesterModal } from './CreateTesterModal';

// The feature-flagged "Docker (local)" provider: the choice appears ONLY when
// GET /api/version reports docker_provider=true, and picking it submits
// cloud "docker" / region "local" with NO cloud_account_id.

function renderWithRouter(ui: ReactNode) {
  return render(<MemoryRouter>{ui}</MemoryRouter>);
}

function mockFetchOnce(body: unknown, status = 200) {
  return Promise.resolve({
    ok: status >= 200 && status < 300,
    status,
    statusText: 'OK',
    headers: new Headers(),
    text: () => Promise.resolve(JSON.stringify(body)),
  } as unknown as Response);
}

const account = {
  account_id: 'acct-1',
  provider: 'azure',
  name: 'prod-azure',
  status: 'active',
  region_default: 'eastus',
  last_validated: new Date().toISOString(),
  validation_error: null,
};

const dockerRow = {
  tester_id: 't-docker',
  project_id: 'p-1',
  name: 'docker-local',
  cloud: 'docker',
  region: 'local',
  vm_size: 'container',
  vm_name: null,
  public_ip: null,
  ssh_user: 'ubuntu',
  power_state: 'provisioning',
  allocation: 'idle',
  status_message: 'starting runner container (docker run)',
  locked_by_config_id: null,
  installer_version: null,
  last_installed_at: null,
  auto_shutdown_enabled: true,
  auto_shutdown_local_hour: 23,
  next_shutdown_at: null,
  shutdown_deferral_count: 0,
  auto_probe_enabled: false,
  last_used_at: null,
  avg_benchmark_duration_seconds: null,
  benchmark_run_count: 0,
  created_by: 'u-1',
  created_at: '2026-08-15T00:00:00Z',
  updated_at: '2026-08-15T00:00:00Z',
};

function stubFetch(dockerProvider: boolean, onCreate?: (body: Record<string, unknown>) => void) {
  const fetchMock = vi.fn((url: string, init?: RequestInit) => {
    if (url.includes('/version')) {
      return mockFetchOnce({
        dashboard_version: '0.28.208', tester_version: null, latest_release: null,
        update_available: false, endpoints: [], docker_provider: dockerProvider,
      });
    }
    if (url.includes('/cloud-accounts')) {
      return mockFetchOnce([account]);
    }
    if (url.endsWith('/testers') && init?.method === 'POST') {
      onCreate?.(JSON.parse(init.body as string));
      return mockFetchOnce(dockerRow, 201);
    }
    if (/\/testers(\?|$)/.test(url)) {
      return mockFetchOnce([]);
    }
    return mockFetchOnce({});
  });
  vi.stubGlobal('fetch', fetchMock);
  return fetchMock;
}

describe('CreateTesterModal — Docker (local) provider', () => {
  beforeEach(() => localStorage.setItem('token', 'test-token'));
  afterEach(() => {
    vi.unstubAllGlobals();
    localStorage.clear();
  });

  it('hides the Docker (local) choice when the control plane has the flag off', async () => {
    stubFetch(false);
    renderWithRouter(<CreateTesterModal projectId="p-1" onCreated={() => {}} onClose={() => {}} />);
    await waitFor(() => expect(screen.getByLabelText(/Region/i)).toBeInTheDocument());
    // Let the version fetch settle, then assert absence.
    await waitFor(() => expect(screen.queryByRole('radio', { name: /Docker \(local\)/i })).toBeNull());
    expect(screen.queryByRole('radiogroup', { name: /Provider/i })).toBeNull();
  });

  it('shows the choice when the flag is on and submits cloud=docker/region=local without an account', async () => {
    let posted: Record<string, unknown> | null = null;
    stubFetch(true, (b) => { posted = b; });
    renderWithRouter(<CreateTesterModal projectId="p-1" onCreated={() => {}} onClose={() => {}} />);

    const dockerRadio = await screen.findByRole('radio', { name: /Docker \(local\)/i });
    // Cloud account is the default; the account list has loaded and pre-selected azure.
    await waitFor(() => expect(screen.getByLabelText(/Region/i)).toHaveValue('eastus'));
    fireEvent.click(dockerRadio);

    expect(dockerRadio).toHaveAttribute('aria-checked', 'true');
    // The account combobox is gone; region is pinned to local and disabled.
    expect(screen.queryByText(/^Cloud Account$/)).toBeNull();
    const region = screen.getByLabelText(/Region/i) as HTMLSelectElement;
    await waitFor(() => expect(region).toHaveValue('local'));
    expect(region).toBeDisabled();
    expect(screen.getByLabelText(/Size/i)).toHaveValue('container');
    expect(screen.getByLabelText(/Operating System/i)).toHaveValue('ubuntu-24.04');

    fireEvent.change(screen.getByLabelText(/Name/i), { target: { value: 'docker-local' } });
    fireEvent.click(screen.getByRole('button', { name: /Create Runner/i }));

    await waitFor(() => expect(posted).not.toBeNull());
    const body = posted as unknown as Record<string, unknown>;
    expect(body.cloud).toBe('docker');
    expect(body.region).toBe('local');
    expect(body.vm_size).toBe('container');
    expect(body.requested_os).toBe('ubuntu-24.04');
    expect(body.cloud_account_id).toBeUndefined();
    await waitFor(() => expect(screen.getByTestId('creating-state')).toBeInTheDocument());
  });

  it('switching back to Cloud account restores the account-backed provider', async () => {
    stubFetch(true);
    renderWithRouter(<CreateTesterModal projectId="p-1" onCreated={() => {}} onClose={() => {}} />);
    fireEvent.click(await screen.findByRole('radio', { name: /Docker \(local\)/i }));
    await waitFor(() => expect(screen.getByLabelText(/Region/i)).toHaveValue('local'));
    fireEvent.click(screen.getByRole('radio', { name: /Cloud account/i }));
    await waitFor(() => expect(screen.getByLabelText(/Region/i)).toHaveValue('eastus'));
    expect(screen.getByLabelText(/Region/i)).toBeEnabled();
    expect(screen.getByLabelText(/VM size/i)).toHaveValue('Standard_B2s');
  });
});
