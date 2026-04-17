import { render, screen, waitFor } from '@testing-library/react';
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { TesterDetailDrawer } from './TesterDetailDrawer';
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
    installer_version: '0.24.2',
    last_installed_at: '2026-04-09T10:00:00Z',
    auto_shutdown_enabled: true,
    auto_shutdown_local_hour: 23,
    next_shutdown_at: '2026-04-11T03:00:00Z',
    shutdown_deferral_count: 0,
    auto_probe_enabled: false,
    last_used_at: '2026-04-10T08:00:00Z',
    avg_benchmark_duration_seconds: 120,
    benchmark_run_count: 5,
    created_by: 'u-1',
    created_at: '2026-04-01T00:00:00Z',
    updated_at: '2026-04-10T08:00:00Z',
    ...overrides,
  };
}

function mockFetch(body: unknown) {
  return Promise.resolve({
    ok: true,
    status: 200,
    statusText: 'OK',
    text: () => Promise.resolve(JSON.stringify(body)),
  } as unknown as Response);
}

describe('TesterDetailDrawer', () => {
  beforeEach(() => {
    localStorage.setItem('token', 'test');
    vi.stubGlobal(
      'fetch',
      vi.fn(() =>
        mockFetch({
          vm_size: 'Standard_D2s_v3',
          hourly_usd: 0.096,
          monthly_always_on_usd: 69.12,
          monthly_with_schedule_usd: 23.04,
          auto_shutdown_enabled: true,
        }),
      ),
    );
    // jsdom doesn't implement WebSocket — stub a no-op.
    vi.stubGlobal(
      'WebSocket',
      class {
        addEventListener() {}
        send() {}
        close() {}
        onclose: (() => void) | null = null;
      },
    );
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    localStorage.clear();
  });

  it('renders the standard sections for an idle tester', async () => {
    render(
      <TesterDetailDrawer
        projectId="p-1"
        tester={baseRow()}
        onClose={() => {}}
        onChanged={() => {}}
      />,
    );
    expect(screen.getByText('Status')).toBeInTheDocument();
    expect(screen.getByText('Identity')).toBeInTheDocument();
    expect(screen.getByText('Version')).toBeInTheDocument();
    expect(screen.getByText('Cost estimate')).toBeInTheDocument();
    expect(screen.getByText('Usage')).toBeInTheDocument();
    expect(screen.getByText('Auto-shutdown')).toBeInTheDocument();
    expect(screen.getByText('Recovery')).toBeInTheDocument();
    expect(screen.getByText('Queue')).toBeInTheDocument();
    expect(screen.getByText('Recent activity')).toBeInTheDocument();
    expect(screen.getByText('Danger zone')).toBeInTheDocument();
    expect(screen.queryByTestId('fix-tester-panel')).toBeNull();
    await waitFor(() =>
      expect(screen.getByText(/\$23\.04/)).toBeInTheDocument(),
    );
  });

  it('shows the fix-tester panel when the tester is in error', () => {
    render(
      <TesterDetailDrawer
        projectId="p-1"
        tester={baseRow({ power_state: 'error', status_message: 'SSH unreachable' })}
        onClose={() => {}}
        onChanged={() => {}}
      />,
    );
    const panel = screen.getByTestId('fix-tester-panel');
    expect(panel).toBeInTheDocument();
    expect(screen.getByText('Fix runner first')).toBeInTheDocument();
    expect(screen.getByText('SSH unreachable')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /^Run probe$/i })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /Reinstall runner/i })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /Force to stopped/i })).toBeInTheDocument();
    // "Mark as healthy" is intentionally absent per spec.
    expect(screen.queryByRole('button', { name: /Mark as healthy/i })).toBeNull();
  });

  it('shows locked-by info when allocation=locked', () => {
    render(
      <TesterDetailDrawer
        projectId="p-1"
        tester={baseRow({
          allocation: 'locked',
          locked_by_config_id: '11111111-2222-3333-4444-555555555555',
        })}
        onClose={() => {}}
        onChanged={() => {}}
      />,
    );
    expect(screen.getByText(/locked by 11111111/)).toBeInTheDocument();
  });
});
