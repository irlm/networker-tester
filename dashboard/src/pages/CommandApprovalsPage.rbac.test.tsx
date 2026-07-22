// RBAC render decisions: CommandApprovalsPage — agent-command approval queue.
//
//   Approve / Deny — PROJECT ADMIN only (approval endpoints are ProjectAdmin
//   server-side). Non-admins keep the read view (the pending queue) with no
//   approve/deny controls.

import { render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { MemoryRouter } from 'react-router-dom';
import { CommandApprovalsPage } from './CommandApprovalsPage';
import { resetRoleStores, setProjectRole, type ProjectRole } from '../test/rbac-helpers';
import type { CommandApproval } from '../api/types';

const approval: CommandApproval = {
  approval_id: 'ap-1',
  project_id: 'p-1',
  agent_id: 'ag-1',
  command_type: 'restart_agent',
  command_detail: {},
  status: 'pending',
  requested_by: 'u-1',
  requested_by_email: 'op@co.com',
  decided_by: null,
  decided_by_email: null,
  requested_at: '2026-07-18T00:00:00Z',
  decided_at: null,
  expires_at: '2099-01-01T00:00:00Z',
  reason: null,
};

vi.mock('../api/client', () => {
  class MockApiError extends Error {
    readonly status: number;
    constructor(status: number, message: string) {
      super(message);
      this.status = status;
    }
  }
  return {
    ApiError: MockApiError,
    api: {
      getPendingApprovals: vi.fn(() => Promise.resolve([approval])),
      decideApproval: vi.fn(() => Promise.resolve()),
    },
  };
});

async function renderPage(role: ProjectRole) {
  setProjectRole(role);
  const utils = render(
    <MemoryRouter initialEntries={['/projects/p-1/approvals']}>
      <CommandApprovalsPage />
    </MemoryRouter>,
  );
  await waitFor(() => expect(screen.getByText('restart_agent')).toBeInTheDocument());
  return utils;
}

describe('CommandApprovalsPage RBAC gating', () => {
  afterEach(resetRoleStores);

  it('viewer: read-only — no approve/deny, queue still shown', async () => {
    await renderPage('viewer');
    expect(screen.queryByRole('button', { name: 'Approve' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Deny' })).not.toBeInTheDocument();
    expect(screen.getByText('restart_agent')).toBeInTheDocument();
  });

  it('operator: still read-only (approvals are admin-only)', async () => {
    await renderPage('operator');
    expect(screen.queryByRole('button', { name: 'Approve' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Deny' })).not.toBeInTheDocument();
  });

  it('admin: approve/deny controls present', async () => {
    await renderPage('admin');
    expect(screen.getByRole('button', { name: 'Approve' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Deny' })).toBeInTheDocument();
  });
});
