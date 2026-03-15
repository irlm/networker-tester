import { useState } from 'react';
import { Link, useLocation } from 'react-router-dom';
import { useAuthStore } from '../../stores/authStore';

const navItems = [
  { path: '/', label: 'Dashboard', icon: '\u25C8' },
  { path: '/agents', label: 'Agents', icon: '\u25C9' },
  { path: '/jobs', label: 'Jobs', icon: '\u25B6' },
  { path: '/runs', label: 'Runs', icon: '\u25F7' },
];

interface SidebarProps {
  connectionDot?: React.ReactNode;
}

export function Sidebar({ connectionDot }: SidebarProps) {
  const location = useLocation();
  const { username, logout } = useAuthStore();
  const [mobileOpen, setMobileOpen] = useState(false);

  return (
    <>
      {/* Mobile toggle button */}
      <button
        onClick={() => setMobileOpen(!mobileOpen)}
        className="fixed top-3 left-3 z-50 md:hidden bg-[#12131a] border border-gray-800 rounded p-2 text-gray-400"
        aria-label="Toggle navigation"
      >
        {mobileOpen ? '\u2715' : '\u2630'}
      </button>

      {/* Sidebar */}
      <aside
        className={`${
          mobileOpen ? 'flex' : 'hidden'
        } md:flex w-56 bg-[#0f1015] border-r border-gray-800 flex-col min-h-screen fixed md:static z-40`}
      >
        <div className="p-4 border-b border-gray-800">
          <div className="flex items-center gap-2">
            <h1 className="text-cyan-400 text-lg font-bold tracking-tight">
              Networker
            </h1>
            {connectionDot}
          </div>
          <p className="text-gray-500 text-xs mt-1">diagnostics platform</p>
        </div>

        <nav className="flex-1 p-2" aria-label="Main navigation">
          {navItems.map((item) => {
            const active = location.pathname === item.path;
            return (
              <Link
                key={item.path}
                to={item.path}
                onClick={() => setMobileOpen(false)}
                aria-current={active ? 'page' : undefined}
                className={`flex items-center gap-3 px-3 py-2 rounded text-sm mb-1 transition-colors ${
                  active
                    ? 'bg-cyan-500/10 text-cyan-400 border border-cyan-500/30'
                    : 'text-gray-400 hover:text-gray-200 hover:bg-gray-800/50'
                }`}
              >
                <span className="text-base" aria-hidden="true">{item.icon}</span>
                {item.label}
              </Link>
            );
          })}
        </nav>

        <div className="p-3 border-t border-gray-800 flex items-center justify-between">
          <span className="text-xs text-gray-500">{username}</span>
          <button
            onClick={logout}
            className="text-xs text-gray-500 hover:text-red-400 transition-colors"
          >
            Logout
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
