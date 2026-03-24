import { useState, useEffect, useCallback } from 'react';
import { Link, useLocation } from 'react-router-dom';
import { useAuthStore } from '../../stores/authStore';
import { useProject } from '../../hooks/useProject';
import { ProjectSwitcher } from '../ProjectSwitcher';
import { NotificationBell } from '../NotificationBell';
import { api } from '../../api/client';

interface NavItem {
  path: string;
  label: string;
  icon: string;
  exact?: boolean;
  badge?: React.ReactNode;
}

interface SidebarProps {
  connectionDot?: React.ReactNode;
}

export function Sidebar({ connectionDot }: SidebarProps) {
  const location = useLocation();
  const { email, role, logout } = useAuthStore();
  const isPlatformAdmin = useAuthStore(s => s.isPlatformAdmin);
  const { projectId, isProjectAdmin, isOperator } = useProject();
  const [mobileOpen, setMobileOpen] = useState(false);
  const [collapsed, setCollapsed] = useState(() => localStorage.getItem('sidebar-collapsed') === '1');
  const [pendingCount, setPendingCount] = useState(0);

  const pid = projectId;
  const isAdmin = role === 'admin' || isPlatformAdmin;

  // ── Nav item groups ─────────────────────────────────────────────────

  const coreItems: NavItem[] = pid ? [
    { path: `/projects/${pid}`, label: 'Dashboard', icon: '\u25C8', exact: true },
    { path: `/projects/${pid}/deploy`, label: 'Infra', icon: '\u25A3' },
    { path: `/projects/${pid}/tests`, label: 'Tests', icon: '\u25B6' },
    { path: `/projects/${pid}/schedules`, label: 'Schedules', icon: '\u21BB' },
    { path: `/projects/${pid}/runs`, label: 'Runs', icon: '\u25F7' },
  ] : [];

  const projectItems: NavItem[] = [];
  if (pid && isOperator) {
    projectItems.push({ path: `/projects/${pid}/cloud-accounts`, label: 'Cloud', icon: '\u2601' });
  }
  if (pid && isProjectAdmin) {
    projectItems.push({ path: `/projects/${pid}/share-links`, label: 'Share Links', icon: '\u{1F517}' });
    projectItems.push({ path: `/projects/${pid}/members`, label: 'Members', icon: '\u2302' });
    projectItems.push({
      path: `/projects/${pid}/approvals`,
      label: 'Approvals',
      icon: '\u2713',
      badge: pid ? <NotificationBell projectId={pid} /> : undefined,
    });
  }

  const platformItems: NavItem[] = [];
  if (isAdmin) {
    platformItems.push({ path: '/users', label: 'Users', icon: '\u265F' });
  }
  if (pid) {
    platformItems.push({ path: `/projects/${pid}/settings`, label: 'Settings', icon: '\u2699' });
  }

  // ── Pending user count (admin only) ─────────────────────────────────

  const fetchPending = useCallback(async () => {
    if (!isAdmin) return;
    try {
      const data = await api.getPendingUsers();
      setPendingCount(data.count);
    } catch {
      // ignore
    }
  }, [isAdmin]);

  useEffect(() => {
    let cancelled = false;
    const run = () => { if (!cancelled) fetchPending(); };
    const id = setInterval(run, 30000);
    void Promise.resolve().then(run);
    return () => { cancelled = true; clearInterval(id); };
  }, [fetchPending]);

  useEffect(() => {
    if (!mobileOpen) return;
    const handler = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setMobileOpen(false);
    };
    document.addEventListener('keydown', handler);
    return () => document.removeEventListener('keydown', handler);
  }, [mobileOpen]);

  // ── Render helpers ──────────────────────────────────────────────────

  const renderItem = (item: NavItem) => {
    const active = item.exact
      ? location.pathname === item.path
      : location.pathname.startsWith(item.path);

    const isUsersWithPending = item.path === '/users' && pendingCount > 0;

    return (
      <Link
        key={item.path}
        to={item.path}
        onClick={() => setMobileOpen(false)}
        aria-current={active ? 'page' : undefined}
        title={collapsed ? item.label : undefined}
        className={`flex items-center overflow-hidden whitespace-nowrap ${collapsed ? 'justify-center' : 'gap-3 px-3'} py-2 rounded text-sm mb-0.5 transition-all duration-200 ${
          active
            ? 'bg-gray-800/40 text-gray-100'
            : 'text-gray-400 hover:text-gray-200 hover:bg-gray-800/30'
        }`}
      >
        <span className="text-base relative" aria-hidden="true">
          {item.icon}
          {isUsersWithPending && collapsed && (
            <span className="absolute -top-1 -right-2 bg-yellow-500 text-[9px] text-black font-bold rounded-full w-3.5 h-3.5 flex items-center justify-center leading-none">
              {pendingCount > 9 ? '9+' : pendingCount}
            </span>
          )}
        </span>
        {!collapsed && (
          <span className="flex items-center gap-2">
            {item.label}
            {isUsersWithPending && (
              <span className="bg-yellow-500 text-[9px] text-black font-bold rounded-full px-1.5 py-0.5 leading-none">
                {pendingCount}
              </span>
            )}
            {item.badge}
          </span>
        )}
      </Link>
    );
  };

  const renderSection = (label: string, items: NavItem[]) => {
    if (items.length === 0) return null;
    return (
      <div className="mt-3 pt-3 border-t border-gray-800/50">
        {!collapsed && (
          <div className="px-3 mb-1.5 text-[10px] uppercase tracking-wider text-gray-600">
            {label}
          </div>
        )}
        {items.map(renderItem)}
      </div>
    );
  };

  return (
    <>
      {/* Mobile toggle button */}
      <button
        onClick={() => setMobileOpen(!mobileOpen)}
        className="fixed top-3 left-3 z-50 md:hidden bg-[var(--bg-sidebar)] border border-gray-800 rounded p-2 text-gray-400"
        aria-label="Toggle navigation"
      >
        {mobileOpen ? '\u2715' : '\u2630'}
      </button>

      {/* Sidebar */}
      <aside
        className={`${
          mobileOpen ? 'flex' : 'hidden'
        } md:flex ${collapsed ? 'w-14' : 'w-48'} bg-[var(--bg-sidebar)] border-r border-gray-800 flex-col min-h-screen fixed md:static z-40 transition-[width] duration-200`}
      >
        <div className={`${collapsed ? 'px-2 py-3' : 'p-4'} border-b border-gray-800`}>
          <ProjectSwitcher collapsed={collapsed} connectionDot={connectionDot} />
        </div>

        <nav className="flex-1 p-1.5 overflow-y-auto" aria-label="Main navigation">
          {/* Core: daily workflow */}
          {coreItems.map(renderItem)}

          {/* Project: admin/operator tools */}
          {renderSection('workspace', projectItems)}

          {/* Platform: global admin */}
          {renderSection('platform', platformItems)}
        </nav>

        {/* Collapse toggle + user */}
        <div className="border-t border-gray-800">
          {!collapsed && (
            <div className="px-3 py-3 flex items-center justify-between">
              <span className="text-xs text-gray-600 truncate max-w-[100px]" title={email ?? ''}>
                {email?.split('@')[0] ?? ''}
              </span>
              <button
                onClick={logout}
                className="text-xs text-gray-500 hover:text-red-400 transition-colors px-2 py-1 rounded hover:bg-gray-800/50"
              >
                Logout
              </button>
            </div>
          )}
          <button
            onClick={() => { const next = !collapsed; setCollapsed(next); localStorage.setItem('sidebar-collapsed', next ? '1' : '0'); }}
            className="hidden md:flex w-full items-center justify-center py-2 text-gray-600 hover:text-gray-400 transition-colors text-xs"
            title={collapsed ? 'Expand sidebar' : 'Collapse sidebar'}
          >
            {collapsed ? '\u25B6' : '\u25C0'}
          </button>
        </div>
      </aside>

      {/* Mobile overlay */}
      {mobileOpen && (
        <div
          className="fixed inset-0 bg-black/50 z-30 md:hidden"
          onClick={() => setMobileOpen(false)}
          aria-hidden="true"
        />
      )}
    </>
  );
}
