import { render, screen } from '@testing-library/react';
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { TesterDetailDrawer } from './TesterDetailDrawer';
import { useProjectStore } from '../stores/projectStore';
import type { TesterRow } from '../api/testers';

function baseRow(overrides: Partial<TesterRow> = {}): TesterRow {
  return {
    tester_id: 't-1',
    project_id: 'p-1',
    name: 'eastus-1',
    cloud: 'azure',
    region: 'eastus',
    vm_size: 'Standard_D2s_v3',
    vm_name: 'nw-t-1',
    public_ip: '1.2.3.4',
    ssh_user: 'networker',
    power_state: 'running',
    allocation: 'idle',
    status_message: null,
    locked_by_config_id: null,
    installer_version: '0.28.46',
    last_installed_at: '2026-07-01T10:00:00Z',
    auto_shutdown_enabled: true,
    auto_shutdown_local_hour: 23,
    next_shutdown_at: '2026-07-11T03:00:00Z',
    shutdown_deferral_count: 0,
    auto_probe_enabled: false,
    last_used_at: '2026-07-10T08:00:00Z',
    api_key_last_used_at: '2026-07-10T08:00:00Z',
    api_key_last_used_ip: '203.0.113.7',
    api_key_expires_at: null,
    avg_benchmark_duration_seconds: 120,
    benchmark_run_count: 5,
    created_by: 'u-1',
    created_at: '2026-07-01T00:00:00Z',
    updated_at: '2026-07-10T08:00:00Z',
    ...overrides,
  };
}

function stubFetch() {
  vi.stubGlobal(
    'fetch',
    vi.fn(() =>
      Promise.resolve({
        ok: true,
        status: 200,
        statusText: 'OK',
        headers: new Headers(),
        text: () =>
          Promise.resolve(
            JSON.stringify({
              vm_size: 'Standard_D2s_v3',
              hourly_usd: 0.096,
              monthly_always_on_usd: 69.12,
              monthly_with_schedule_usd: 23.04,
              auto_shutdown_enabled: true,
            }),
          ),
      } as unknown as Response),
    ),
  );
  vi.stubGlobal(
    'WebSocket',
    class {
      addEventListener() {}
      send() {}
      close() {}
      onclose: (() => void) | null = null;
    },
  );
}

function renderDrawer(role: 'operator' | 'viewer', row: TesterRow) {
  useProjectStore.setState({ activeProjectRole: role });
  return render(
    <TesterDetailDrawer
      projectId="p-1"
      tester={row}
      onClose={() => {}}
      onChanged={() => {}}
    />,
  );
}

describe('TesterDetailDrawer — agent key section', () => {
  beforeEach(() => {
    localStorage.setItem('token', 'test');
    stubFetch();
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    localStorage.clear();
    useProjectStore.setState({ activeProjectRole: null });
  });

  it('operator sees the Rotate key control', () => {
    renderDrawer('operator', baseRow());
    expect(screen.getByText('Agent key')).toBeInTheDocument();
    expect(
      screen.getByRole('button', { name: /rotate key/i }),
    ).toBeInTheDocument();
  });

  it('viewer sees the key section read-only — no Rotate control', () => {
    renderDrawer('viewer', baseRow());
    // The read-only "Agent key" section still renders (last seen / expiry),
    // but the mutating Rotate control is operator-gated.
    expect(screen.getByText('Agent key')).toBeInTheDocument();
    expect(
      screen.queryByRole('button', { name: /rotate key/i }),
    ).not.toBeInTheDocument();
  });

  it('renders "never" when the key has never been used', () => {
    renderDrawer('operator', baseRow({ api_key_last_used_at: null }));
    expect(screen.getByText('Last seen:')).toBeInTheDocument();
    expect(screen.getByText('never')).toBeInTheDocument();
  });

  it('shows an expired badge when the key expiry is in the past', () => {
    renderDrawer(
      'operator',
      baseRow({ api_key_expires_at: '2020-01-01T00:00:00Z' }),
    );
    expect(screen.getByText('expired')).toBeInTheDocument();
  });

  it('shows "no expiry" when api_key_expires_at is null', () => {
    renderDrawer('operator', baseRow({ api_key_expires_at: null }));
    expect(screen.getByText('no expiry')).toBeInTheDocument();
  });
});
