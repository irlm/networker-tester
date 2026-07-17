// RBAC render decisions: TesterDetailDrawer — tester start/stop/delete controls.
//
// Viewers get a read-only drawer: no Danger zone (Start/Stop/Delete runner),
// no schedule mutation buttons, no probe trigger, and no error-recovery
// actions. Operators and project admins get the full control surface.

import { render, screen } from '@testing-library/react';
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { TesterDetailDrawer } from './TesterDetailDrawer';
import type { TesterRow } from '../api/testers';
import { setProjectRole, resetRoleStores, type ProjectRole } from '../test/rbac-helpers';

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
    power_state: 'stopped',
    allocation: 'idle',
    status_message: null,
    locked_by_config_id: null,
    installer_version: '0.28.34',
    last_installed_at: '2026-07-01T10:00:00Z',
    auto_shutdown_enabled: true,
    auto_shutdown_local_hour: 23,
    next_shutdown_at: null,
    shutdown_deferral_count: 0,
    auto_probe_enabled: false,
    last_used_at: null,
    avg_benchmark_duration_seconds: null,
    benchmark_run_count: 0,
    created_by: 'u-1',
    created_at: '2026-07-01T00:00:00Z',
    updated_at: '2026-07-01T00:00:00Z',
    ...overrides,
  };
}

function renderDrawer(role: ProjectRole, tester: TesterRow = baseRow()) {
  setProjectRole(role);
  return render(
    <TesterDetailDrawer
      projectId="p-1"
      tester={tester}
      onClose={() => {}}
      onChanged={() => {}}
    />,
  );
}

describe('TesterDetailDrawer role gating (tester start/stop/delete)', () => {
  beforeEach(() => {
    localStorage.setItem('token', 'test');
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
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    localStorage.clear();
    resetRoleStores();
  });

  it('viewer: read-only — no Danger zone, no start/stop/delete', () => {
    renderDrawer('viewer');
    // Read-only sections stay visible.
    expect(screen.getByText('Status')).toBeInTheDocument();
    expect(screen.getByText('Auto-shutdown')).toBeInTheDocument();
    // Mutating controls are gone.
    expect(screen.queryByText('Danger zone')).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /Start runner/i })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /Stop runner/i })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /Delete runner/i })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /Edit schedule/i })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /Postpone 2h/i })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /Run probe now/i })).not.toBeInTheDocument();
  });

  it('viewer: error-state drawer shows the fault but no recovery actions', () => {
    renderDrawer('viewer', baseRow({ power_state: 'error', status_message: 'SSH unreachable' }));
    expect(screen.getByText('Fix runner first')).toBeInTheDocument();
    expect(screen.getByText('SSH unreachable')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /^Run probe$/i })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /Reinstall runner/i })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /Force to stopped/i })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /Delete runner/i })).not.toBeInTheDocument();
  });

  it('operator: full control surface (start + delete on a stopped runner)', () => {
    renderDrawer('operator');
    expect(screen.getByText('Danger zone')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /Start runner/i })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /Delete runner/i })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /Edit schedule/i })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /Run probe now/i })).toBeInTheDocument();
  });

  it('operator: running runner shows Stop instead of Start', () => {
    renderDrawer('operator', baseRow({ power_state: 'running' }));
    expect(screen.getByRole('button', { name: /Stop runner/i })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /Start runner/i })).not.toBeInTheDocument();
  });

  it('admin: same full control surface as operator (admin ⊇ operator)', () => {
    renderDrawer('admin');
    expect(screen.getByText('Danger zone')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /Start runner/i })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /Delete runner/i })).toBeInTheDocument();
  });
});
