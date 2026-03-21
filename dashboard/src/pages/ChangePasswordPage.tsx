import { useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { api } from '../api/client';
import { useAuthStore } from '../stores/authStore';

export function ChangePasswordPage() {
  const [currentPassword, setCurrentPassword] = useState('');
  const [newPassword, setNewPassword] = useState('');
  const [confirmPassword, setConfirmPassword] = useState('');
  const [email, setEmail] = useState('');
  const [error, setError] = useState('');
  const [loading, setLoading] = useState(false);
  const mustChangePassword = useAuthStore((s) => s.mustChangePassword);
  const clearPasswordChange = useAuthStore((s) => s.clearPasswordChange);
  const navigate = useNavigate();

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    setError('');

    if (newPassword.length < 8) {
      setError('New password must be at least 8 characters');
      return;
    }

    if (newPassword !== confirmPassword) {
      setError('New passwords do not match');
      return;
    }

    if (currentPassword === newPassword) {
      setError('New password must be different from current password');
      return;
    }

    if (mustChangePassword && !email.trim()) {
      setError('Email is required for password recovery');
      return;
    }

    if (email && !/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(email)) {
      setError('Please enter a valid email address');
      return;
    }

    setLoading(true);
    try {
      await api.changePassword(
        currentPassword,
        newPassword,
        email.trim() || undefined,
      );
      clearPasswordChange();
      navigate('/');
    } catch {
      setError('Failed to change password. Check your current password and try again.');
    } finally {
      setLoading(false);
    }
  };

  return (
    <div className="min-h-screen bg-[var(--bg-base)] flex items-center justify-center p-4">
      <div className="w-full max-w-xs">
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
          <div className="text-yellow-400 text-xs mb-6 flex items-center gap-2">
            <span className="text-yellow-500">warn</span>
            <span>{mustChangePassword ? 'Set your password and recovery email' : 'Change password'}</span>
          </div>

          {error && (
            <div className="text-red-400 text-xs mb-4 flex items-center gap-2">
              <span className="text-red-500">err</span>
              <span>{error}</span>
            </div>
          )}

          <div className="mb-4">
            <label htmlFor="current-password" className="block text-xs text-gray-600 mb-1.5 uppercase tracking-wider">
              Current password
            </label>
            <div className="flex items-center border-b border-gray-700 focus-within:border-green-500/50 transition-colors">
              <span className="text-green-600/60 text-sm mr-2 select-none">&gt;</span>
              <input
                id="current-password"
                type="password"
                value={currentPassword}
                onChange={(e) => setCurrentPassword(e.target.value)}
                className="w-full bg-transparent py-2 text-sm text-gray-200 focus:outline-none placeholder:text-gray-700"
                autoFocus
              />
            </div>
          </div>

          <div className="mb-4">
            <label htmlFor="new-password" className="block text-xs text-gray-600 mb-1.5 uppercase tracking-wider">
              New password
            </label>
            <div className="flex items-center border-b border-gray-700 focus-within:border-green-500/50 transition-colors">
              <span className="text-green-600/60 text-sm mr-2 select-none">&gt;</span>
              <input
                id="new-password"
                type="password"
                value={newPassword}
                onChange={(e) => setNewPassword(e.target.value)}
                className="w-full bg-transparent py-2 text-sm text-gray-200 focus:outline-none placeholder:text-gray-700"
                placeholder="min 8 characters"
              />
            </div>
          </div>

          <div className="mb-4">
            <label htmlFor="confirm-password" className="block text-xs text-gray-600 mb-1.5 uppercase tracking-wider">
              Confirm password
            </label>
            <div className="flex items-center border-b border-gray-700 focus-within:border-green-500/50 transition-colors">
              <span className="text-green-600/60 text-sm mr-2 select-none">&gt;</span>
              <input
                id="confirm-password"
                type="password"
                value={confirmPassword}
                onChange={(e) => setConfirmPassword(e.target.value)}
                className="w-full bg-transparent py-2 text-sm text-gray-200 focus:outline-none placeholder:text-gray-700"
              />
            </div>
          </div>

          {/* Email — required on first login, optional on subsequent changes */}
          <div className="mb-8">
            <label htmlFor="recovery-email" className="block text-xs text-gray-600 mb-1.5 uppercase tracking-wider">
              Recovery email {mustChangePassword ? '' : '(optional)'}
            </label>
            <div className="flex items-center border-b border-gray-700 focus-within:border-green-500/50 transition-colors">
              <span className="text-green-600/60 text-sm mr-2 select-none">&gt;</span>
              <input
                id="recovery-email"
                type="email"
                value={email}
                onChange={(e) => setEmail(e.target.value)}
                className="w-full bg-transparent py-2 text-sm text-gray-200 focus:outline-none placeholder:text-gray-700"
                placeholder="you@example.com"
              />
            </div>
            <p className="text-xs text-gray-700 mt-1">
              Used to send a reset link if you forget your password
            </p>
          </div>

          <button
            type="submit"
            disabled={loading}
            className="w-full bg-green-600 hover:bg-green-500 text-white py-2 rounded text-sm font-medium transition-colors disabled:opacity-50"
          >
            {loading ? 'Updating...' : 'Set password'}
          </button>
        </form>
      </div>
    </div>
  );
}
