// RBAC render decisions: Sidebar — which admin nav sections each role sees.
//
//   Users                       — account admin OR platform admin
//   System / Tokens / Perf Log  — platform admin only
//   Settings                    — anyone with an active project
//   Core nav (Dashboard, Runs)  — anyone with an active project

import { render, screen } from '@testing-library/react';
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { MemoryRouter } from 'react-router-dom';
import { Sidebar } from './Sidebar';
import { setProjectRole, setAuthRole, resetRoleStores } from '../../test/rbac-helpers';

// Child components with their own data dependencies are out of scope here —
// this suite targets the nav gating decisions only.
vi.mock('../ProjectSwitcher', () => ({
  ProjectSwitcher: () => <div data-testid="project-switcher" />,
}));
vi.mock('../NotificationBell', () => ({
  NotificationBell: () => <span data-testid="notification-bell" />,
}));
vi.mock('../../api/client', () => ({
  api: {
    getPendingUsers: vi.fn(() => Promise.resolve({ count: 0 })),
  },
}));

function renderSidebar() {
  return render(
    <MemoryRouter initialEntries={['/projects/p-1']}>
      <Sidebar />
    </MemoryRouter>,
  );
}

describe('Sidebar role gating', () => {
  beforeEach(() => {
    // Admin group is collapsible; open it so gating (not collapse state)
    // decides visibility.
    localStorage.setItem('sidebar-admin-open', '1');
  });

  afterEach(() => {
    localStorage.clear();
    resetRoleStores();
  });

  it('viewer (regular account): project nav but no admin entries', () => {
    setAuthRole('user');
    setProjectRole('viewer');
    renderSidebar();
    expect(screen.getByText('Dashboard')).toBeInTheDocument();
    expect(screen.getByText('Runs')).toBeInTheDocument();
    expect(screen.getByText('Settings')).toBeInTheDocument();
    expect(screen.queryByText('Users')).not.toBeInTheDocument();
    expect(screen.queryByText('System')).not.toBeInTheDocument();
    expect(screen.queryByText('Tokens')).not.toBeInTheDocument();
    expect(screen.queryByText('Perf Log')).not.toBeInTheDocument();
  });

  it('operator (regular account): same nav as viewer — operator adds page-level controls, not nav', () => {
    setAuthRole('user');
    setProjectRole('operator');
    renderSidebar();
    expect(screen.getByText('Settings')).toBeInTheDocument();
    expect(screen.queryByText('Users')).not.toBeInTheDocument();
    expect(screen.queryByText('System')).not.toBeInTheDocument();
  });

  it('account admin: Users appears, platform-admin entries stay hidden', () => {
    setAuthRole('admin');
    setProjectRole('admin');
    renderSidebar();
    expect(screen.getByText('Users')).toBeInTheDocument();
    expect(screen.queryByText('System')).not.toBeInTheDocument();
    expect(screen.queryByText('Tokens')).not.toBeInTheDocument();
    expect(screen.queryByText('Perf Log')).not.toBeInTheDocument();
  });

  it('platform admin: full admin nav (System, Tokens, Perf Log, Users)', () => {
    setAuthRole('user', true); // platform admin overrides account role
    setProjectRole('admin');
    renderSidebar();
    expect(screen.getByText('System')).toBeInTheDocument();
    expect(screen.getByText('Tokens')).toBeInTheDocument();
    expect(screen.getByText('Perf Log')).toBeInTheDocument();
    expect(screen.getByText('Users')).toBeInTheDocument();
  });
});
