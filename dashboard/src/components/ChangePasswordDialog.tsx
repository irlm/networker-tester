import { useState } from 'react';
import { api } from '../api/client';
import { useAuthStore } from '../stores/authStore';
import { Modal } from './common/Modal';

interface ChangePasswordDialogProps {
  onClose: () => void;
}

export function ChangePasswordDialog({ onClose }: ChangePasswordDialogProps) {
  const [currentPassword, setCurrentPassword] = useState('');
  const [newPassword, setNewPassword] = useState('');
  const [confirmPassword, setConfirmPassword] = useState('');
  const [error, setError] = useState('');
  const [loading, setLoading] = useState(false);
  const [success, setSuccess] = useState(false);
  const clearPasswordChange = useAuthStore((s) => s.clearPasswordChange);

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

    setLoading(true);
    try {
      await api.changePassword(currentPassword, newPassword);
      clearPasswordChange();
      setSuccess(true);
      setTimeout(onClose, 1200);
    } catch {
      setError('Failed to change password. Check your current password and try again.');
    } finally {
      setLoading(false);
    }
  };

  return (
    <Modal onClose={onClose} labelledBy="change-pw-title" maxWidth="max-w-sm">
        <div className="flex items-center justify-between mb-5">
          <h2 id="change-pw-title" className="text-sm font-semibold text-gray-200">Change password</h2>
          <button
            onClick={onClose}
            className="text-faint hover:text-gray-400 transition-colors text-xs px-1"
            aria-label="Close"
          >
            Esc
          </button>
        </div>

        {success ? (
          <div className="text-green-400 text-xs flex items-center gap-2 py-4">
            <span className="text-green-500">ok</span>
            <span>Password updated successfully</span>
          </div>
        ) : (
          <form onSubmit={handleSubmit}>
            {error && (
              <div className="text-red-400 text-xs mb-4 flex items-center gap-2">
                <span className="text-red-500">err</span>
                <span>{error}</span>
              </div>
            )}

            <div className="mb-4">
              <label htmlFor="dlg-current-password" className="block text-xs text-faint mb-1.5 uppercase tracking-wider">
                Current password
              </label>
              <input
                id="dlg-current-password"
                type="password"
                value={currentPassword}
                onChange={(e) => setCurrentPassword(e.target.value)}
                className="w-full bg-transparent border-b border-gray-700 focus:border-cyan-600 py-2 text-sm text-gray-200 focus:outline-none placeholder:text-gray-700 transition-colors"
                autoComplete="current-password"
              />
            </div>

            <div className="mb-4">
              <label htmlFor="dlg-new-password" className="block text-xs text-faint mb-1.5 uppercase tracking-wider">
                New password
              </label>
              <input
                id="dlg-new-password"
                type="password"
                value={newPassword}
                onChange={(e) => setNewPassword(e.target.value)}
                className="w-full bg-transparent border-b border-gray-700 focus:border-cyan-600 py-2 text-sm text-gray-200 focus:outline-none placeholder:text-gray-700 transition-colors"
                placeholder="min 8 characters"
                autoComplete="new-password"
              />
            </div>

            <div className="mb-6">
              <label htmlFor="dlg-confirm-password" className="block text-xs text-faint mb-1.5 uppercase tracking-wider">
                Confirm password
              </label>
              <input
                id="dlg-confirm-password"
                type="password"
                value={confirmPassword}
                onChange={(e) => setConfirmPassword(e.target.value)}
                className="w-full bg-transparent border-b border-gray-700 focus:border-cyan-600 py-2 text-sm text-gray-200 focus:outline-none placeholder:text-gray-700 transition-colors"
                autoComplete="new-password"
              />
            </div>

            <div className="flex justify-end gap-2">
              <button
                type="button"
                onClick={onClose}
                className="text-xs text-gray-400 hover:text-gray-300 px-3 py-2 transition-colors"
              >
                Cancel
              </button>
              <button
                type="submit"
                disabled={loading || !currentPassword || !newPassword || !confirmPassword}
                className="bg-cyan-700 hover:bg-cyan-600 disabled:opacity-40 text-[var(--bg-base)] text-xs px-4 py-2 rounded transition-colors"
              >
                {loading ? 'Updating...' : 'Update password'}
              </button>
            </div>
          </form>
        )}
    </Modal>
  );
}
