import { useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { api } from '../api/client';
import { useAuthStore } from '../stores/authStore';

export function ChangePasswordPage() {
  const [currentPassword, setCurrentPassword] = useState('');
  const [newPassword, setNewPassword] = useState('');
  const [confirmPassword, setConfirmPassword] = useState('');
  const [error, setError] = useState('');
  const [loading, setLoading] = useState(false);
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

    setLoading(true);
    try {
      await api.changePassword(currentPassword, newPassword);
      clearPasswordChange();
      navigate('/');
    } catch {
      setError('Failed to change password. Check your current password and try again.');
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
        <p className="text-gray-500 text-sm mb-6">Change your password</p>

        <div className="text-yellow-400 text-sm bg-yellow-500/10 border border-yellow-500/30 rounded p-2 mb-4">
          You must change your password before continuing
        </div>

        {error && (
          <p className="text-red-400 text-sm bg-red-500/10 border border-red-500/30 rounded p-2 mb-4">
            {error}
          </p>
        )}

        <label htmlFor="current-password" className="block text-xs text-gray-400 mb-1">Current password</label>
        <input
          id="current-password"
          type="password"
          value={currentPassword}
          onChange={(e) => setCurrentPassword(e.target.value)}
          className="w-full bg-[#0a0b0f] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 mb-4 focus:outline-none focus:border-cyan-500"
          autoFocus
        />

        <label htmlFor="new-password" className="block text-xs text-gray-400 mb-1">New password</label>
        <input
          id="new-password"
          type="password"
          value={newPassword}
          onChange={(e) => setNewPassword(e.target.value)}
          className="w-full bg-[#0a0b0f] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 mb-4 focus:outline-none focus:border-cyan-500"
        />

        <label htmlFor="confirm-password" className="block text-xs text-gray-400 mb-1">Confirm new password</label>
        <input
          id="confirm-password"
          type="password"
          value={confirmPassword}
          onChange={(e) => setConfirmPassword(e.target.value)}
          className="w-full bg-[#0a0b0f] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 mb-6 focus:outline-none focus:border-cyan-500"
        />

        <button
          type="submit"
          disabled={loading}
          className="w-full bg-cyan-600 hover:bg-cyan-500 text-white py-2 rounded text-sm font-medium transition-colors disabled:opacity-50"
        >
          {loading ? 'Changing password...' : 'Change password'}
        </button>
      </form>
    </div>
  );
}
