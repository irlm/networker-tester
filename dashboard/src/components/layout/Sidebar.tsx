import { useState, useEffect, useCallback } from 'react';
import { Link, useLocation } from 'react-router-dom';
import { useAuthStore } from '../../stores/authStore';
import { api } from '../../api/client';

const navItems = [
  { path: '/', label: 'Dashboard', icon: '\u25C8' },
  { path: '/deploy', label: 'Infra', icon: '\u25A3' },
  { path: '/tests', label: 'Tests', icon: '\u25B6' },
  { path: '/schedules', label: 'Schedules', icon: '\u21BB' },
  { path: '/runs', label: 'Runs', icon: '\u25F7' },
];

interface SidebarProps {
  connectionDot?: React.ReactNode;
}

export function Sidebar({ connectionDot }: SidebarProps) {
  const location = useLocation();
  const { email, role, logout } = useAuthStore();
  const [mobileOpen, setMobileOpen] = useState(false);
  const [collapsed, setCollapsed] = useState(() => localStorage.getItem('sidebar-collapsed') === '1');
  const [pendingCount, setPendingCount] = useState(0);

  const fetchPending = useCallback(async () => {
    if (role !== 'admin') return;
    try {
      const data = await api.getPendingUsers();
      setPendingCount(data.count);
    } catch {
      // ignore
    }
  }, [role]);

  useEffect(() => {
    let cancelled = false;
    const run = () => { if (!cancelled) fetchPending(); };
    const id = setInterval(run, 30000);
    // Schedule initial fetch as microtask to avoid synchronous setState in effect
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

  const isAdmin = role === 'admin';

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
          {collapsed ? (
            <div className="flex justify-center">
              <span className="text-green-400 text-lg font-bold">A</span>
            </div>
          ) : (
            <>
              <div className="flex items-center gap-2">
                <h1 className="text-green-400 text-lg font-bold tracking-tight">
                  AletheDash
                </h1>
                {connectionDot}
              </div>
              <p className="text-gray-600 text-xs mt-0.5">diagnostics</p>
            </>
          )}
        </div>

        <nav className="flex-1 p-1.5" aria-label="Main navigation">
          {navItems.map((item) => {
            const active = item.path === '/'
              ? location.pathname === '/'
              : location.pathname.startsWith(item.path);
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
                <span className="text-base" aria-hidden="true">{item.icon}</span>
                {!collapsed && item.label}
              </Link>
            );
          })}
          {isAdmin && (() => {
            const active = location.pathname.startsWith('/users');
            return (
              <Link
                to="/users"
                onClick={() => setMobileOpen(false)}
                aria-current={active ? 'page' : undefined}
                title={collapsed ? 'Users' : undefined}
                className={`flex items-center overflow-hidden whitespace-nowrap ${collapsed ? 'justify-center' : 'gap-3 px-3'} py-2 rounded text-sm mb-0.5 transition-all duration-200 ${
                  active
                    ? 'bg-gray-800/40 text-gray-100'
                    : 'text-gray-400 hover:text-gray-200 hover:bg-gray-800/30'
                }`}
              >
                <span className="text-base relative" aria-hidden="true">
                  {'\u265F'}
                  {pendingCount > 0 && (
                    <span className="absolute -top-1 -right-2 bg-yellow-500 text-[9px] text-black font-bold rounded-full w-3.5 h-3.5 flex items-center justify-center leading-none">
                      {pendingCount > 9 ? '9+' : pendingCount}
                    </span>
                  )}
                </span>
                {!collapsed && (
                  <span className="flex items-center gap-2">
                    Users
                    {pendingCount > 0 && (
                      <span className="bg-yellow-500 text-[9px] text-black font-bold rounded-full px-1.5 py-0.5 leading-none">
                        {pendingCount}
                      </span>
                    )}
                  </span>
                )}
              </Link>
            );
          })()}
          {(() => {
            const active = location.pathname.startsWith('/settings');
            return (
              <Link
                to="/settings"
                onClick={() => setMobileOpen(false)}
                aria-current={active ? 'page' : undefined}
                title={collapsed ? 'Settings' : undefined}
                className={`flex items-center overflow-hidden whitespace-nowrap ${collapsed ? 'justify-center' : 'gap-3 px-3'} py-2 rounded text-sm mb-0.5 transition-all duration-200 ${
                  active
                    ? 'bg-gray-800/40 text-gray-100'
                    : 'text-gray-400 hover:text-gray-200 hover:bg-gray-800/30'
                }`}
              >
                <span className="text-base" aria-hidden="true">{'\u2699'}</span>
                {!collapsed && 'Settings'}
              </Link>
            );
          })()}
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
