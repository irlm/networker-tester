// RBAC render decisions: ProjectMembersPage — workspace member management.
//
//   Invite / Import CSV / Add-existing / role <select> / Remove / Revoke /
//   Send-invite / bulk-send / pending checkboxes — PROJECT ADMIN only
//   (members endpoints are ProjectAdmin server-side).
//
// Non-admins keep the full read view (member list with role as a badge) but get
// zero mutation entry points — closing the deep-link gap where a viewer saw
// fully-wired invite/remove/role-change forms.

import { render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { MemoryRouter } from 'react-router-dom';
import { ProjectMembersPage } from './ProjectMembersPage';
import { resetRoleStores, setProjectRole, type ProjectRole } from '../test/rbac-helpers';
import type { ProjectMember } from '../api/types';

const member: ProjectMember = {
  project_id: 'p-1',
  user_id: 'u-1',
  role: 'operator',
  status: 'active',
  joined_at: '2026-07-18T00:00:00Z',
  invited_by: null,
  invite_sent_at: null,
  email: 'alice@co.com',
  display_name: 'Alice',
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
      getProjectMembers: vi.fn(() => Promise.resolve([member])),
      getInvites: vi.fn(() => Promise.resolve([])),
    },
  };
});

async function renderPage(role: ProjectRole) {
  setProjectRole(role);
  const utils = render(
    <MemoryRouter initialEntries={['/projects/p-1/members']}>
      <ProjectMembersPage />
    </MemoryRouter>,
  );
  // Assert gating against real loaded content, not the loading state.
  await waitFor(() => expect(screen.getByText('alice@co.com')).toBeInTheDocument());
  return utils;
}

describe('ProjectMembersPage RBAC gating', () => {
  afterEach(resetRoleStores);

  it('viewer: read-only — no invite/import/role-change/remove, list still shown', async () => {
    await renderPage('viewer');
    expect(screen.queryByRole('button', { name: 'Invite' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Import CSV' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Remove' })).not.toBeInTheDocument();
    expect(screen.queryByRole('combobox')).not.toBeInTheDocument(); // no role <select>
    // The member row is still visible (role rendered as a read-only badge).
    expect(screen.getByText('alice@co.com')).toBeInTheDocument();
  });

  it('operator: still read-only (member management is admin-only)', async () => {
    await renderPage('operator');
    expect(screen.queryByRole('button', { name: 'Invite' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Remove' })).not.toBeInTheDocument();
    expect(screen.queryByRole('combobox')).not.toBeInTheDocument();
  });

  it('admin: full mutation surface — invite/import/role-change/remove', async () => {
    await renderPage('admin');
    expect(screen.getByRole('button', { name: 'Invite' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Import CSV' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Remove' })).toBeInTheDocument();
    expect(screen.getByRole('combobox')).toBeInTheDocument(); // role <select> for the member
  });
});
