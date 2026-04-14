import { render, screen, waitFor, fireEvent } from '@testing-library/react';
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { CreateTesterModal } from './CreateTesterModal';

type FetchMock = ReturnType<typeof vi.fn>;

const sampleRow = {
  tester_id: 't-1',
  project_id: 'p-1',
  name: 'eastus-1',
  cloud: 'azure',
  region: 'eastus',
  vm_size: 'Standard_D2s_v3',
  vm_name: null,
  public_ip: null,
  ssh_user: 'networker',
  power_state: 'provisioning',
  allocation: 'idle',
  status_message: 'Provisioning VM…',
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
  created_at: '2026-04-10T00:00:00Z',
  updated_at: '2026-04-10T00:00:00Z',
};

function mockFetchOnce(body: unknown, status = 200) {
  return Promise.resolve({
    ok: status >= 200 && status < 300,
    status,
    statusText: 'OK',
    text: () => Promise.resolve(JSON.stringify(body)),
  } as unknown as Response);
}

describe('CreateTesterModal', () => {
  let fetchMock: FetchMock;

  beforeEach(() => {
    localStorage.setItem('token', 'test-token');
    fetchMock = vi.fn();
    // Default: regions fetch on mount.
    fetchMock.mockImplementation((url: string) => {
      if (url.includes('/testers/regions')) {
        return mockFetchOnce({ regions: ['eastus', 'westus'] });
      }
      if (url.includes('/cloud-connections') || url.includes('/cloud-accounts')) {
        return mockFetchOnce([]);
      }
      if (/\/testers(\?|$)/.test(url)) {
        return mockFetchOnce([]);
      }
      return mockFetchOnce({});
    });
    vi.stubGlobal('fetch', fetchMock);
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    localStorage.clear();
  });

  it('renders the form with default values', async () => {
    render(
      <CreateTesterModal
        projectId="p-1"
        defaultRegion="eastus"
        defaultName="eastus-1"
        onCreated={() => {}}
        onClose={() => {}}
      />,
    );
    await waitFor(() => {
      expect(screen.getByLabelText(/Region/i)).toBeInTheDocument();
    });
    expect(screen.getByLabelText(/Name/i)).toHaveValue('eastus-1');
    expect(screen.getByLabelText(/VM size/i)).toHaveValue('Standard_D2s_v3');
    expect(screen.getByRole('button', { name: /Create Tester/i })).toBeEnabled();
  });

  it('POSTs create body and enters creating state', async () => {
    // Second call (after regions) = POST /testers returns provisioning row.
    fetchMock.mockImplementation((url: string, init?: RequestInit) => {
      if (url.includes('/testers/regions')) {
        return mockFetchOnce({ regions: ['eastus'] });
      }
      if (url.endsWith('/testers') && init?.method === 'POST') {
        const body = JSON.parse(init.body as string);
        expect(body.cloud).toBe('azure');
        expect(body.region).toBe('eastus');
        expect(body.name).toBe('eastus-1');
        expect(body.vm_size).toBe('Standard_D2s_v3');
        expect(body.auto_shutdown_local_hour).toBe(23);
        expect(body.auto_probe_enabled).toBe(false);
        return mockFetchOnce(sampleRow, 201);
      }
      return mockFetchOnce({});
    });

    const onCreated = vi.fn();
    render(
      <CreateTesterModal
        projectId="p-1"
        defaultRegion="eastus"
        defaultName="eastus-1"
        onCreated={onCreated}
        onClose={() => {}}
      />,
    );

    await waitFor(() =>
      expect(screen.getByRole('button', { name: /Create Tester/i })).toBeEnabled(),
    );
    fireEvent.click(screen.getByRole('button', { name: /Create Tester/i }));

    await waitFor(() =>
      expect(screen.getByTestId('creating-state')).toBeInTheDocument(),
    );
    expect(screen.getByText(/Provisioning VM/)).toBeInTheDocument();
  });
});
