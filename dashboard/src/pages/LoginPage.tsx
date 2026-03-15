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
      login(res.token, res.username, res.role);
      navigate('/');
    } catch {
      setError('Invalid credentials');
    } finally {
      setLoading(false);
    }
  };

  return (
    <div className="min-h-screen flex items-center justify-center bg-[#0a0b0f]">
      <form
        onSubmit={handleSubmit}
        className="w-96 bg-[#12131a] border border-gray-800 rounded-lg p-8"
      >
        <h1 className="text-cyan-400 text-xl font-bold mb-1">Networker</h1>
        <p className="text-gray-500 text-sm mb-6">Sign in to the dashboard</p>

        {error && (
          <p className="text-red-400 text-sm bg-red-500/10 border border-red-500/30 rounded p-2 mb-4">
            {error}
          </p>
        )}

        <label htmlFor="login-username" className="block text-xs text-gray-400 mb-1">Username</label>
        <input
          id="login-username"
          type="text"
          value={username}
          onChange={(e) => setUsername(e.target.value)}
          className="w-full bg-[#0a0b0f] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 mb-4 focus:outline-none focus:border-cyan-500"
          autoFocus
        />

        <label htmlFor="login-password" className="block text-xs text-gray-400 mb-1">Password</label>
        <input
          id="login-password"
          type="password"
          value={password}
          onChange={(e) => setPassword(e.target.value)}
          className="w-full bg-[#0a0b0f] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 mb-6 focus:outline-none focus:border-cyan-500"
        />

        <button
          type="submit"
          disabled={loading}
          className="w-full bg-cyan-600 hover:bg-cyan-500 text-white py-2 rounded text-sm font-medium transition-colors disabled:opacity-50"
        >
          {loading ? 'Signing in...' : 'Sign in'}
        </button>
      </form>
    </div>
  );
}
