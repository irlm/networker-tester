import { useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { api } from '../api/client';
import { useAuthStore } from '../stores/authStore';

export function LoginPage() {
  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');
  const [error, setError] = useState('');
  const [loading, setLoading] = useState(false);
  const login = useAuthStore((s) => s.login);
  const navigate = useNavigate();

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    setError('');
    setLoading(true);
    try {
      const res = await api.login(username, password);
      login(res.token, res.username, res.role, res.must_change_password);
      if (res.must_change_password) {
        navigate('/change-password');
      } else {
        navigate('/');
      }
    } catch {
      setError('Invalid credentials');
    } finally {
      setLoading(false);
    }
  };

  return (
    <div className="min-h-screen bg-[var(--bg-base)] flex items-center justify-center">
      <div className="w-72">
        {/* Brand */}
        <div className="text-center mb-8">
          <h1 className="text-[#4ade80] text-2xl font-bold tracking-tight mb-1">
            Networker
          </h1>
          <p className="text-gray-600 text-xs uppercase tracking-widest">
            diagnostics platform
          </p>
        </div>

        {/* Form */}
        <form onSubmit={handleSubmit}>
          {error && (
            <div className="text-red-400 text-xs mb-4 flex items-center gap-2">
              <span className="text-red-500">err</span>
              <span>{error}</span>
            </div>
          )}

          <div className="mb-4">
            <label htmlFor="login-username" className="block text-xs text-gray-600 mb-1.5 uppercase tracking-wider">
              Username
            </label>
            <div className="flex items-center border-b border-gray-700 focus-within:border-green-500/50 transition-colors">
              <span className="text-green-600/60 text-sm mr-2 select-none">&gt;</span>
              <input
                id="login-username"
                type="text"
                value={username}
                onChange={(e) => setUsername(e.target.value)}
                className="w-full bg-transparent py-2 text-sm text-gray-200 focus:outline-none placeholder:text-gray-700"
                placeholder="admin"
                autoFocus
              />
            </div>
          </div>

          <div className="mb-8">
            <label htmlFor="login-password" className="block text-xs text-gray-600 mb-1.5 uppercase tracking-wider">
              Password
            </label>
            <div className="flex items-center border-b border-gray-700 focus-within:border-green-500/50 transition-colors">
              <span className="text-green-600/60 text-sm mr-2 select-none">&gt;</span>
              <input
                id="login-password"
                type="password"
                value={password}
                onChange={(e) => setPassword(e.target.value)}
                className="w-full bg-transparent py-2 text-sm text-gray-200 focus:outline-none placeholder:text-gray-700"
                placeholder="••••••••"
              />
            </div>
          </div>

          <button
            type="submit"
            disabled={loading}
            className="w-full bg-green-600 hover:bg-green-500 text-white py-2 rounded text-sm font-medium transition-colors disabled:opacity-50"
          >
            {loading ? 'Authenticating...' : 'Sign in'}
          </button>
        </form>
      </div>
    </div>
  );
}
