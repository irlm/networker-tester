import { useEffect, useCallback } from 'react';
import { useNavigate } from 'react-router-dom';
import { api } from '../api/client';
import { useAuthStore } from '../stores/authStore';

export function PendingPage() {
  const email = useAuthStore((s) => s.email);
  const logout = useAuthStore((s) => s.logout);
  const updateStatus = useAuthStore((s) => s.updateStatus);
  const navigate = useNavigate();

  const pollStatus = useCallback(async () => {
    try {
      const profile = await api.getProfile();
      if (profile.status === 'active') {
        updateStatus('active');
        navigate('/', { replace: true });
      }
    } catch {
      // ignore -- will retry on next poll
    }
  }, [updateStatus, navigate]);

  useEffect(() => {
    pollStatus();
    const id = setInterval(pollStatus, 10000);
    return () => clearInterval(id);
  }, [pollStatus]);

  const handleLogout = () => {
    logout();
    navigate('/login', { replace: true });
  };

  return (
    <div className="min-h-screen bg-[var(--bg-base)] flex items-center justify-center">
      <div className="w-80 text-center">
        {/* Brand */}
        <div className="mb-8">
          <h1 className="text-[#4ade80] text-2xl font-bold tracking-tight mb-1">
            AletheDash
          </h1>
          <p className="text-gray-600 text-xs uppercase tracking-widest">
            network diagnostics
          </p>
        </div>

        {/* Status icon */}
        <div className="flex justify-center mb-6">
          <div className="w-16 h-16 rounded-full border-2 border-yellow-500/50 flex items-center justify-center">
            <span className="text-2xl" role="img" aria-label="Pending">&#9203;</span>
          </div>
        </div>

        {/* Heading */}
        <h2 className="text-gray-100 text-lg font-semibold mb-2">
          Account pending approval
        </h2>

        {/* Signed in as */}
        <p className="text-green-400 text-sm font-mono mb-6">
          Signed in as {email}
        </p>

        {/* Status card */}
        <div className="border border-gray-800 rounded bg-[var(--bg-card)] px-4 py-3 mb-6">
          <div className="flex items-center gap-2 justify-center">
            <span className="w-2 h-2 rounded-full bg-yellow-400 motion-safe:animate-pulse" />
            <span className="text-sm text-gray-400">Waiting for admin approval</span>
          </div>
        </div>

        {/* Sign out */}
        <button
          onClick={handleLogout}
          className="px-4 py-2 text-sm text-gray-400 hover:text-gray-200 border border-gray-700 hover:border-gray-600 rounded transition-colors"
        >
          Sign out
        </button>
      </div>
    </div>
  );
}
