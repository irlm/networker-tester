// RBAC render decisions: ShareLinksPage — external share-link management.
//
//   Revoke / +30d (extend) / Delete — PROJECT ADMIN only (share-link endpoints
//   are ProjectAdmin server-side). Non-admins keep the read view (the link
//   table) with no mutation controls.

import { render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { MemoryRouter } from 'react-router-dom';
import { ShareLinksPage } from './ShareLinksPage';
import { resetRoleStores, setProjectRole, type ProjectRole } from '../test/rbac-helpers';
import type { ShareLink } from '../api/types';

const link: ShareLink = {
  link_id: 'l-1',
  resource_type: 'run',
  resource_id: 'r-1',
  label: 'prod dashboard',
  expires_at: '2099-01-01T00:00:00Z', // active (future)
  created_by: 'u-1',
  created_by_email: 'admin@co.com',
  created_at: '2026-07-18T00:00:00Z',
  revoked: false,
  access_count: 3,
  last_accessed: null,
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
      getShareLinks: vi.fn(() => Promise.resolve([link])),
      revokeShareLink: vi.fn(() => Promise.resolve()),
      extendShareLink: vi.fn(() => Promise.resolve()),
      deleteShareLink: vi.fn(() => Promise.resolve()),
    },
  };
});

async function renderPage(role: ProjectRole) {
  setProjectRole(role);
  const utils = render(
    <MemoryRouter initialEntries={['/projects/p-1/share-links']}>
      <ShareLinksPage />
    </MemoryRouter>,
  );
  await waitFor(() => expect(screen.getByText('prod dashboard')).toBeInTheDocument());
  return utils;
}

describe('ShareLinksPage RBAC gating', () => {
  afterEach(resetRoleStores);

  it('viewer: read-only — no revoke/extend/delete, table still shown', async () => {
    await renderPage('viewer');
    expect(screen.queryByRole('button', { name: 'Revoke' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: '+30d' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Delete' })).not.toBeInTheDocument();
    expect(screen.getByText('prod dashboard')).toBeInTheDocument();
  });

  it('operator: still read-only (share links are admin-only)', async () => {
    await renderPage('operator');
    expect(screen.queryByRole('button', { name: 'Revoke' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Delete' })).not.toBeInTheDocument();
  });

  it('admin: full mutation surface — revoke/extend/delete', async () => {
    await renderPage('admin');
    expect(screen.getByRole('button', { name: 'Revoke' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '+30d' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Delete' })).toBeInTheDocument();
  });
});
