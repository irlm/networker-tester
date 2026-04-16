import { render, screen, fireEvent } from '@testing-library/react';
import { describe, it, expect, vi } from 'vitest';
import { TesterRegionGroup } from './TesterRegionGroup';
import type { TesterRow } from '../api/testers';

function row(overrides: Partial<TesterRow> = {}): TesterRow {
  return {
    tester_id: 't-1',
    project_id: 'p-1',
    name: 'eastus-1',
    cloud: 'azure',
    region: 'eastus',
    vm_size: 'Standard_D2s_v3',
    vm_name: null,
    public_ip: null,
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
    avg_benchmark_duration_seconds: null,
    benchmark_run_count: 0,
    created_by: 'u-1',
    created_at: '2026-04-01T00:00:00Z',
    updated_at: '2026-04-10T00:00:00Z',
    ...overrides,
  };
}

describe('TesterRegionGroup', () => {
  it('renders header with counts and calls onSelect when a row is clicked', () => {
    const onSelect = vi.fn();
    const onAdd = vi.fn();
    render(
      <TesterRegionGroup
        cloud="azure"
        region="eastus"
        testers={[row(), row({ tester_id: 't-2', name: 'eastus-2' })]}
        queues={{}}
        onSelect={onSelect}
        onAdd={onAdd}
      />,
    );
    expect(screen.getByText(/azure \/ eastus/)).toBeInTheDocument();
    expect(screen.getByText(/2 runners/)).toBeInTheDocument();
    fireEvent.click(screen.getByText('eastus-1'));
    expect(onSelect).toHaveBeenCalledTimes(1);

    fireEvent.click(screen.getByRole('button', { name: /\+ add to eastus/ }));
    expect(onAdd).toHaveBeenCalledWith('azure', 'eastus');
  });
});
