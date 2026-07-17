// RBAC render decisions: SettingsTabs — which settings sections each role sees.
//
//   General     — everyone with a project
//   Cloud       — operator+ (cloud credentials management)
//   Members / Share Links / Approvals — project admin only

import { render, screen } from '@testing-library/react';
import { describe, it, expect, afterEach } from 'vitest';
import { MemoryRouter } from 'react-router-dom';
import { SettingsTabs } from './SettingsTabs';
import { setProjectRole, resetRoleStores, type ProjectRole } from '../../test/rbac-helpers';

function renderFor(role: ProjectRole) {
  setProjectRole(role);
  return render(
    <MemoryRouter initialEntries={['/projects/p-1/settings']}>
      <SettingsTabs />
    </MemoryRouter>,
  );
}

describe('SettingsTabs role gating', () => {
  afterEach(resetRoleStores);

  it('viewer: General only — no admin sections, no Cloud', () => {
    renderFor('viewer');
    expect(screen.getByText('General')).toBeInTheDocument();
    expect(screen.queryByText('Cloud')).not.toBeInTheDocument();
    expect(screen.queryByText('Members')).not.toBeInTheDocument();
    expect(screen.queryByText('Share Links')).not.toBeInTheDocument();
    expect(screen.queryByText('Approvals')).not.toBeInTheDocument();
  });

  it('operator: General + Cloud — still no admin sections', () => {
    renderFor('operator');
    expect(screen.getByText('General')).toBeInTheDocument();
    expect(screen.getByText('Cloud')).toBeInTheDocument();
    expect(screen.queryByText('Members')).not.toBeInTheDocument();
    expect(screen.queryByText('Share Links')).not.toBeInTheDocument();
    expect(screen.queryByText('Approvals')).not.toBeInTheDocument();
  });

  it('admin: all five sections', () => {
    renderFor('admin');
    expect(screen.getByText('General')).toBeInTheDocument();
    expect(screen.getByText('Cloud')).toBeInTheDocument();
    expect(screen.getByText('Members')).toBeInTheDocument();
    expect(screen.getByText('Share Links')).toBeInTheDocument();
    expect(screen.getByText('Approvals')).toBeInTheDocument();
  });
});
