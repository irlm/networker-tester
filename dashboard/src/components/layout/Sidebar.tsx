import { Link, useLocation } from 'react-router-dom';
import { useAuthStore } from '../../stores/authStore';

const navItems = [
  { path: '/', label: 'Dashboard', icon: '◈' },
  { path: '/agents', label: 'Agents', icon: '◉' },
  { path: '/jobs', label: 'Jobs', icon: '▶' },
  { path: '/runs', label: 'Runs', icon: '◷' },
];

export function Sidebar() {
  const location = useLocation();
  const { username, logout } = useAuthStore();

  return (
    <aside className="w-56 bg-[#0f1015] border-r border-gray-800 flex flex-col min-h-screen">
      <div className="p-4 border-b border-gray-800">
        <h1 className="text-cyan-400 text-lg font-bold tracking-tight">
          Networker
        </h1>
        <p className="text-gray-500 text-xs mt-1">diagnostics platform</p>
      </div>

      <nav className="flex-1 p-2">
        {navItems.map((item) => {
          const active = location.pathname === item.path;
          return (
            <Link
              key={item.path}
              to={item.path}
              className={`flex items-center gap-3 px-3 py-2 rounded text-sm mb-1 transition-colors ${
                active
                  ? 'bg-cyan-500/10 text-cyan-400 border border-cyan-500/30'
                  : 'text-gray-400 hover:text-gray-200 hover:bg-gray-800/50'
              }`}
            >
              <span className="text-base">{item.icon}</span>
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
  );
}
