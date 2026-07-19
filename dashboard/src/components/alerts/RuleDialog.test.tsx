// Rule builder dialog: inline validation before any API call, and the exact
// snake_case payload shape on submit (docs/alerting.md rule body).

import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { RuleDialog } from './RuleDialog';
import type { AlertChannel, AlertRule, TestConfigListItem } from '../../api/types';

vi.mock('../../api/client', () => ({
  api: {
    createAlertRule: vi.fn(() => Promise.resolve({})),
    updateAlertRule: vi.fn(() => Promise.resolve({})),
  },
}));

import { api } from '../../api/client';

const channels: AlertChannel[] = [
  {
    channel_id: 'ch-1',
    project_id: 'p-1',
    kind: 'webhook',
    name: 'ops hook',
    config: { url: 'https://hooks.example.com/x' },
    enabled: true,
    created_at: '2026-07-18T00:00:00Z',
  },
];

const configs: TestConfigListItem[] = [
  {
    id: 'cfg-1',
    project_id: 'p-1',
    name: 'prod-api-latency',
    endpoint_kind: 'network',
    modes: ['http1'],
    has_methodology: false,
    created_at: '2026-07-18T00:00:00Z',
    updated_at: '2026-07-18T00:00:00Z',
  },
];

function renderDialog(existing: AlertRule | null = null) {
  return render(
    <RuleDialog
      projectId="p-1"
      channels={channels}
      configs={configs}
      existing={existing}
      onClose={() => {}}
      onSaved={() => {}}
    />,
  );
}

describe('RuleDialog', () => {
  afterEach(() => vi.clearAllMocks());

  it('shows a validation error instead of calling the API when threshold is missing', async () => {
    const user = userEvent.setup();
    renderDialog();
    await user.click(screen.getByRole('button', { name: 'Create Rule' }));
    expect(await screen.findByRole('alert')).toHaveTextContent(/finite number/i);
    expect(api.createAlertRule).not.toHaveBeenCalled();
  });

  it('rejects an out-of-range window before submitting', async () => {
    const user = userEvent.setup();
    renderDialog();
    await user.type(screen.getByLabelText(/Threshold/), '500');
    const windowInput = screen.getByLabelText(/Consecutive runs/);
    await user.clear(windowInput);
    await user.type(windowInput, '99');
    await user.click(screen.getByRole('button', { name: 'Create Rule' }));
    expect(await screen.findByRole('alert')).toHaveTextContent(/between 1 and 50/i);
    expect(api.createAlertRule).not.toHaveBeenCalled();
  });

  it('submits the snake_case rule body for a config-scoped rule', async () => {
    const user = userEvent.setup();
    renderDialog();
    await user.selectOptions(screen.getByLabelText('Metric'), 'error_rate');
    await user.selectOptions(screen.getByLabelText('Comparator'), 'gt');
    await user.type(screen.getByLabelText(/Threshold/), '0.05');
    const windowInput = screen.getByLabelText(/Consecutive runs/);
    await user.clear(windowInput);
    await user.type(windowInput, '3');
    await user.selectOptions(screen.getByLabelText('Scope'), 'cfg-1');
    await user.click(screen.getByRole('button', { name: 'Create Rule' }));

    await waitFor(() => expect(api.createAlertRule).toHaveBeenCalledTimes(1));
    expect(api.createAlertRule).toHaveBeenCalledWith('p-1', {
      metric: 'error_rate',
      comparator: 'gt',
      threshold: 0.05,
      window_runs: 3,
      channel_id: 'ch-1',
      test_config_id: 'cfg-1',
      enabled: true,
    });
  });

  it('omits test_config_id for project-wide rules', async () => {
    const user = userEvent.setup();
    renderDialog();
    await user.type(screen.getByLabelText(/Threshold/), '500');
    await user.click(screen.getByRole('button', { name: 'Create Rule' }));

    await waitFor(() => expect(api.createAlertRule).toHaveBeenCalledTimes(1));
    const body = vi.mocked(api.createAlertRule).mock.calls[0][1];
    expect(body).not.toHaveProperty('test_config_id');
    expect(body).toMatchObject({ metric: 'p95_ms', comparator: 'gt', threshold: 500, window_runs: 1 });
  });

  it('edits via PATCH and locks a config-scoped rule out of the project-wide option', async () => {
    const user = userEvent.setup();
    const existing: AlertRule = {
      rule_id: 'r-1',
      project_id: 'p-1',
      test_config_id: 'cfg-1',
      metric: 'p95_ms',
      comparator: 'gt',
      threshold: 500,
      window_runs: 3,
      enabled: true,
      channel_id: 'ch-1',
      created_by: null,
      created_at: '2026-07-18T00:00:00Z',
    };
    renderDialog(existing);

    // Project-wide option is disabled — PATCH cannot clear the scope.
    expect(screen.getByRole('option', { name: 'All configs in project' })).toBeDisabled();

    const thresholdInput = screen.getByLabelText(/Threshold/);
    await user.clear(thresholdInput);
    await user.type(thresholdInput, '750');
    await user.click(screen.getByRole('button', { name: 'Save Rule' }));

    await waitFor(() => expect(api.updateAlertRule).toHaveBeenCalledTimes(1));
    expect(api.updateAlertRule).toHaveBeenCalledWith('r-1', expect.objectContaining({ threshold: 750 }));
    expect(api.createAlertRule).not.toHaveBeenCalled();
  });
});
