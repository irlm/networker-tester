// RBAC render decisions: InfrastructurePage — deploy/runner wizard access.
//
//   "+ Deploy", "+ runner", "+ target", "Deploy your first target" — operator+
//   "+ Create your first runner …" (runner empty-state CTA)         — project admin
//
// Viewers get the data (stats, target/runner tables) with zero mutation
// entry points.

import { render, screen, waitFor } from '@testing-library/react';
import { describe, it, expect, vi, afterEach } from 'vitest';
import { MemoryRouter } from 'react-router-dom';
import { InfrastructurePage } from './InfrastructurePage';
import { setProjectRole, resetRoleStores, type ProjectRole } from '../test/rbac-helpers';

vi.mock('../api/client', () => ({
  api: {
    getDeployments: vi.fn(() => Promise.resolve([])),
    getCloudAccounts: vi.fn(() => Promise.resolve([])),
  },
}));
vi.mock('../api/testers', () => ({
  testersApi: {
    listTesters: vi.fn(() => Promise.resolve([])),
  },
}));
vi.mock('../api/vmHistory', () => ({
  listVmHistory: vi.fn(() => Promise.resolve({ events: [] })),
}));
// Live-queue WebSocket subscription is out of scope for gating decisions.
vi.mock('../hooks/useTesterSubscription', () => ({
  useTesterSubscription: () => ({}),
}));

async function renderPage(role: ProjectRole) {
  setProjectRole(role);
  const utils = render(
    <MemoryRouter initialEntries={['/projects/p-1/vms']}>
      <InfrastructurePage />
    </MemoryRouter>,
  );
  // Wait for the initial load to settle so gating is asserted on real content.
  await waitFor(() =>
    expect(screen.getByText(/No targets deployed/i)).toBeInTheDocument(),
  );
  return utils;
}

describe('InfrastructurePage wizard access gating', () => {
  afterEach(resetRoleStores);

  it('viewer: zero wizard entry points', async () => {
    await renderPage('viewer');
    expect(screen.queryByRole('button', { name: '+ Deploy' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: '+ runner' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: '+ target' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /Deploy your first target/i })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /Create your first runner/i })).not.toBeInTheDocument();
  });

  it('operator: deploy/target wizards available, runner-creation CTA still admin-only', async () => {
    await renderPage('operator');
    expect(screen.getByRole('button', { name: '+ Deploy' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '+ runner' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '+ target' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /Deploy your first target/i })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /Create your first runner/i })).not.toBeInTheDocument();
  });

  it('admin: full provisioning surface including the runner empty-state CTA', async () => {
    await renderPage('admin');
    expect(screen.getByRole('button', { name: '+ Deploy' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /Create your first runner/i })).toBeInTheDocument();
  });
});
