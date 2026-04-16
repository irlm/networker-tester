import { render, screen, waitFor } from '@testing-library/react';
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { TesterStep } from './TesterStep';

// WebSocket is used by useTesterSubscription. We stub it out so the hook is
// inert during these unit tests — we only care about REST-driven rendering.
class NoopWebSocket {
  addEventListener() {}
  send() {}
  close() {}
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  set onclose(_: any) {}
}

const idleTester = {
  tester_id: 't-idle',
  project_id: 'p-1',
  name: 'eastus-1',
  cloud: 'azure',
  region: 'eastus',
  vm_size: 'Standard_D2s_v3',
  vm_name: 'eastus-1-vm',
  public_ip: '1.2.3.4',
  ssh_user: 'networker',
  power_state: 'running',
  allocation: 'idle',
  status_message: null,
  locked_by_config_id: null,
  installer_version: '0.24.2',
  last_installed_at: null,
  auto_shutdown_enabled: true,
  auto_shutdown_local_hour: 23,
  next_shutdown_at: null,
  shutdown_deferral_count: 0,
  auto_probe_enabled: false,
  last_used_at: null,
  avg_benchmark_duration_seconds: 600,
  benchmark_run_count: 5,
  created_by: 'u-1',
  created_at: '2026-04-10T00:00:00Z',
  updated_at: '2026-04-10T00:00:00Z',
};

function mockFetchResponse(body: unknown, status = 200) {
  return Promise.resolve({
    ok: status >= 200 && status < 300,
    status,
    statusText: 'OK',
    text: () => Promise.resolve(JSON.stringify(body)),
  } as unknown as Response);
}

describe('TesterStep', () => {
  let fetchMock: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    localStorage.setItem('token', 'test-token');
    fetchMock = vi.fn();
    vi.stubGlobal('fetch', fetchMock);
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    vi.stubGlobal('WebSocket', NoopWebSocket as any);
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    localStorage.clear();
  });

  it('State A: renders radio list when matching testers exist', async () => {
    fetchMock.mockImplementation((url: string) => {
      if (url.includes('/testers')) {
        return mockFetchResponse([idleTester]);
      }
      return mockFetchResponse([]);
    });

    render(
      <TesterStep
        projectId="p-1"
        cloud="Azure"
        region="eastus"
        value={null}
        onChange={() => {}}
      />,
    );

    await waitFor(() => {
      expect(screen.getByRole('radiogroup')).toBeTruthy();
    });
    expect(screen.getByText('eastus-1')).toBeTruthy();
    expect(screen.getByText('idle')).toBeTruthy();
  });

  it('State B: shows create CTA when no testers match the region', async () => {
    fetchMock.mockImplementation((url: string) => {
      if (url.includes('/testers')) {
        return mockFetchResponse([]);
      }
      return mockFetchResponse([]);
    });

    render(
      <TesterStep
        projectId="p-1"
        cloud="Azure"
        region="eastus"
        value={null}
        onChange={() => {}}
      />,
    );

    await waitFor(() => {
      expect(screen.getByText(/No runners in/)).toBeTruthy();
    });
    expect(screen.getByText('Create eastus runner')).toBeTruthy();
  });

  it('State A filter: excludes testers from other regions', async () => {
    const westTester = { ...idleTester, tester_id: 't-w', name: 'westus-1', region: 'westus' };
    fetchMock.mockImplementation((url: string) => {
      if (url.includes('/testers')) {
        return mockFetchResponse([idleTester, westTester]);
      }
      return mockFetchResponse([]);
    });

    render(
      <TesterStep
        projectId="p-1"
        cloud="Azure"
        region="eastus"
        value={null}
        onChange={() => {}}
      />,
    );

    await waitFor(() => {
      expect(screen.getByText('eastus-1')).toBeTruthy();
    });
    expect(screen.queryByText('westus-1')).toBeNull();
  });

  it('renders busy warning when selected tester is locked', async () => {
    const busyTester = {
      ...idleTester,
      tester_id: 't-busy',
      name: 'busy-1',
      allocation: 'locked',
    };
    fetchMock.mockImplementation((url: string) => {
      if (url.includes('/testers')) {
        return mockFetchResponse([busyTester]);
      }
      return mockFetchResponse([]);
    });

    render(
      <TesterStep
        projectId="p-1"
        cloud="Azure"
        region="eastus"
        value="t-busy"
        onChange={() => {}}
      />,
    );

    await waitFor(() => {
      expect(screen.getByText('busy-1')).toBeTruthy();
    });
    expect(screen.getByText(/currently busy/)).toBeTruthy();
  });
});
