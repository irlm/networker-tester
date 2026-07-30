// The testers LIST endpoint is a slim projection — no vm_name / public_ip /
// ssh_user / created_by / installer_version / OS facts. The drawer must fetch
// the DETAIL record and merge it over the polled row, or those fields render
// as em dashes forever (user report 2026-07-30: "where is the OS of the
// current already running servers?" — every detail-only field showed "—").

import { render, screen, waitFor } from '@testing-library/react';
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { TesterDetailDrawer } from './TesterDetailDrawer';
import type { TesterRow } from '../api/testers';
import { setProjectRole, resetRoleStores } from '../test/rbac-helpers';

/** What the LIST actually returns: the slim projection (detail-only fields absent). */
function slimRow(): TesterRow {
  return {
    tester_id: 't-1',
    project_id: 'p-1',
    name: 'eastus-runner-01',
    cloud: 'azure',
    region: 'eastus',
    vm_size: 'Standard_B2s',
    power_state: 'running',
    allocation: 'idle',
    status_message: null,
    auto_shutdown_enabled: true,
    auto_shutdown_local_hour: 23,
    next_shutdown_at: null,
    shutdown_deferral_count: 0,
    last_used_at: null,
    created_at: '2026-07-29T15:44:00Z',
    updated_at: '2026-07-29T15:44:00Z',
  } as unknown as TesterRow;
}

const DETAIL = {
  ...slimRow(),
  vm_name: 'tester-eastus-59ea5',
  public_ip: '20.106.131.109',
  ssh_user: 'azureuser',
  created_by: 'user-1',
  installer_version: '0.28.109',
  last_installed_at: '2026-07-29T15:50:00Z',
  requested_os: 'linux',
  os_distro: 'ubuntu',
  os_version: '24.04',
  os_arch: 'x86_64',
  auto_probe_enabled: false,
  avg_benchmark_duration_seconds: null,
  benchmark_run_count: 0,
  locked_by_config_id: null,
};

const COST = {
  vm_size: 'Standard_B2s',
  hourly_usd: 0.042,
  monthly_always_on_usd: 29.95,
  monthly_with_schedule_usd: 18.72,
  auto_shutdown_enabled: true,
};

describe('TesterDetailDrawer detail fetch', () => {
  beforeEach(() => {
    localStorage.setItem('token', 'test');
    setProjectRole('viewer');
    vi.stubGlobal(
      'fetch',
      vi.fn((input: RequestInfo | URL) => {
        const url = String(input);
        const body = url.includes('cost_estimate') ? COST : DETAIL;
        return Promise.resolve({
          ok: true,
          status: 200,
          statusText: 'OK',
          headers: new Headers(),
          text: () => Promise.resolve(JSON.stringify(body)),
        } as unknown as Response);
      }),
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
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    localStorage.clear();
    resetRoleStores();
  });

  it('renders detail-only fields (OS, VM name, IP, version) fetched from the detail endpoint', async () => {
    render(
      <TesterDetailDrawer
        projectId="p-1"
        tester={slimRow()}
        onClose={() => {}}
        onChanged={() => {}}
      />,
    );

    // Detail-only fields appear once GET /testers/{id} resolves.
    await waitFor(() => {
      expect(screen.getByText('ubuntu 24.04 (x86_64)')).toBeInTheDocument();
    });
    expect(screen.getByText('tester-eastus-59ea5')).toBeInTheDocument();
    expect(screen.getByText('20.106.131.109')).toBeInTheDocument();
    expect(screen.getByText('azureuser')).toBeInTheDocument();
    expect(screen.getByText('0.28.109')).toBeInTheDocument();
  });

  it('falls back to the requested OS when discovered facts are absent', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn((input: RequestInfo | URL) => {
        const url = String(input);
        const body = url.includes('cost_estimate')
          ? COST
          : { ...DETAIL, os_distro: null, os_version: null, os_arch: null };
        return Promise.resolve({
          ok: true,
          status: 200,
          statusText: 'OK',
          headers: new Headers(),
          text: () => Promise.resolve(JSON.stringify(body)),
        } as unknown as Response);
      }),
    );

    render(
      <TesterDetailDrawer
        projectId="p-1"
        tester={slimRow()}
        onClose={() => {}}
        onChanged={() => {}}
      />,
    );

    await waitFor(() => {
      expect(screen.getByText('linux (requested)')).toBeInTheDocument();
    });
  });
});
