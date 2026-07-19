// RBAC render decisions: AlertsPage — alert rule/channel mutation surface.
//
//   "+ Rule", "+ Channel", enable toggles, edit/delete, channel Test-fire — operator+
//
// Viewers get the full data (rules, channels, history) with zero mutation
// entry points; toggles render as read-only status badges instead.

import { render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { MemoryRouter } from 'react-router-dom';
import { AlertsPage } from './AlertsPage';
import { resetRoleStores, setProjectRole, type ProjectRole } from '../test/rbac-helpers';
import type { AlertChannel, AlertRule } from '../api/types';

const rule: AlertRule = {
  rule_id: 'r-1',
  project_id: 'p-1',
  test_config_id: null,
  metric: 'p95_ms',
  comparator: 'gt',
  threshold: 500,
  window_runs: 3,
  enabled: true,
  channel_id: 'ch-1',
  created_by: null,
  created_at: '2026-07-18T00:00:00Z',
};

const channel: AlertChannel = {
  channel_id: 'ch-1',
  project_id: 'p-1',
  kind: 'webhook',
  name: 'ops hook',
  config: { url: 'https://hooks.example.com/x', secret: '********' },
  enabled: true,
  created_at: '2026-07-18T00:00:00Z',
};

vi.mock('../api/client', () => {
  class MockApiError extends Error {
    readonly status: number;
    readonly body: string | null;
    constructor(status: number, message: string, body: string | null = null) {
      super(message);
      this.status = status;
      this.body = body;
    }
  }
  return {
    ApiError: MockApiError,
    api: {
      listAlertRules: vi.fn(() => Promise.resolve([rule])),
      listAlertChannels: vi.fn(() => Promise.resolve([channel])),
      listTestConfigs: vi.fn(() => Promise.resolve([])),
      listAlertEvents: vi.fn(() => Promise.resolve([])),
    },
  };
});

async function renderPage(role: ProjectRole, tab: 'rules' | 'channels' = 'rules') {
  setProjectRole(role);
  const utils = render(
    <MemoryRouter initialEntries={[`/projects/p-1/alerts${tab === 'channels' ? '?tab=channels' : ''}`]}>
      <AlertsPage />
    </MemoryRouter>,
  );
  // Wait for the initial load so gating is asserted on real content.
  await waitFor(() =>
    expect(screen.getByText(tab === 'rules' ? 'p95_ms > 500ms' : 'ops hook')).toBeInTheDocument(),
  );
  return utils;
}

describe('AlertsPage mutation gating', () => {
  afterEach(resetRoleStores);

  it('viewer: rules tab is read-only — no create/edit/delete/toggle', async () => {
    await renderPage('viewer');
    expect(screen.queryByRole('button', { name: '+ Rule' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'edit' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /Disable rule|Enable rule/ })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /Create your first rule/i })).not.toBeInTheDocument();
    // Enabled state still visible, as a read-only badge.
    expect(screen.getByText('on')).toBeInTheDocument();
  });

  it('viewer: channels tab is read-only — no create/test/edit/delete/toggle', async () => {
    await renderPage('viewer', 'channels');
    expect(screen.queryByRole('button', { name: '+ Channel' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Test' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'edit' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /Disable channel|Enable channel/ })).not.toBeInTheDocument();
    // Destination stays visible (secret already masked by the API).
    expect(screen.getByText('https://hooks.example.com/x')).toBeInTheDocument();
  });

  it('operator: full rules mutation surface', async () => {
    await renderPage('operator');
    expect(screen.getByRole('button', { name: '+ Rule' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'edit' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Disable rule' })).toBeInTheDocument();
  });

  it('operator: full channels mutation surface including Test-fire', async () => {
    await renderPage('operator', 'channels');
    expect(screen.getByRole('button', { name: '+ Channel' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Test' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'edit' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Disable channel' })).toBeInTheDocument();
  });

  it('admin: same mutation surface as operator', async () => {
    await renderPage('admin');
    expect(screen.getByRole('button', { name: '+ Rule' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Disable rule' })).toBeInTheDocument();
  });
});
