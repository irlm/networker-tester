import { useState } from 'react';
import { Link, useSearchParams } from 'react-router-dom';
import { api } from '../api/client';

export function ResetPasswordPage() {
  const [searchParams] = useSearchParams();
  const token = searchParams.get('token') || '';
  const [newPassword, setNewPassword] = useState('');
  const [confirmPassword, setConfirmPassword] = useState('');
  const [error, setError] = useState('');
  const [success, setSuccess] = useState(false);
  const [loading, setLoading] = useState(false);

  if (!token) {
    return (
      <div className="min-h-screen bg-[var(--bg-base)] flex items-center justify-center p-4">
        <div className="w-full max-w-xs text-center">
          <p className="text-red-400 text-sm mb-4">Invalid reset link — no token provided.</p>
          <Link to="/forgot-password" className="text-xs text-cyan-400 hover:text-cyan-300">
            Request a new reset link
          </Link>
        </div>
      </div>
    );
  }

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    setError('');

    if (newPassword.length < 8) {
      setError('Password must be at least 8 characters');
      return;
    }
    if (newPassword !== confirmPassword) {
      setError('Passwords do not match');
      return;
    }

    setLoading(true);
    try {
      await api.resetPassword(token, newPassword);
      setSuccess(true);
    } catch (err) {
      const msg = err instanceof Error ? err.message : 'Reset failed';
      setError(msg);
    } finally {
      setLoading(false);
    }
  };

  return (
    <div className="min-h-screen bg-[var(--bg-base)] flex items-center justify-center p-4">
      <div className="w-full max-w-xs">
        <div className="text-center mb-8">
          <h1 className="text-[#4ade80] text-2xl font-bold tracking-tight mb-1">
            Networker
          </h1>
          <p className="text-gray-600 text-xs uppercase tracking-widest">
            diagnostics platform
          </p>
        </div>

        {success ? (
          <div>
            <div className="text-green-400 text-xs mb-4 flex items-center gap-2">
              <span className="text-green-500">ok</span>
              <span>Password has been reset</span>
            </div>
            <Link
              to="/login"
              className="block w-full bg-green-600 hover:bg-green-500 text-white py-2 rounded text-sm font-medium transition-colors text-center"
            >
              Sign in
            </Link>
          </div>
        ) : (
          <form onSubmit={handleSubmit}>
            <p className="text-gray-400 text-xs mb-6">
              Choose a new password for your account.
            </p>

            {error && (
              <div className="text-red-400 text-xs mb-4 flex items-center gap-2">
                <span className="text-red-500">err</span>
                <span>{error}</span>
              </div>
            )}

            <div className="mb-4">
              <label htmlFor="reset-new-password" className="block text-xs text-gray-600 mb-1.5 uppercase tracking-wider">
                New password
              </label>
              <div className="flex items-center border-b border-gray-700 focus-within:border-green-500/50 transition-colors">
                <span className="text-green-600/60 text-sm mr-2 select-none">&gt;</span>
                <input
                  id="reset-new-password"
                  type="password"
                  value={newPassword}
                  onChange={(e) => setNewPassword(e.target.value)}
                  className="w-full bg-transparent py-2 text-sm text-gray-200 focus:outline-none placeholder:text-gray-700"
                  placeholder="min 8 characters"
                  autoFocus
                />
              </div>
            </div>

            <div className="mb-8">
              <label htmlFor="reset-confirm-password" className="block text-xs text-gray-600 mb-1.5 uppercase tracking-wider">
                Confirm password
              </label>
              <div className="flex items-center border-b border-gray-700 focus-within:border-green-500/50 transition-colors">
                <span className="text-green-600/60 text-sm mr-2 select-none">&gt;</span>
                <input
                  id="reset-confirm-password"
                  type="password"
                  value={confirmPassword}
                  onChange={(e) => setConfirmPassword(e.target.value)}
                  className="w-full bg-transparent py-2 text-sm text-gray-200 focus:outline-none placeholder:text-gray-700"
                />
              </div>
            </div>

            <button
              type="submit"
              disabled={loading}
              className="w-full bg-green-600 hover:bg-green-500 text-white py-2 rounded text-sm font-medium transition-colors disabled:opacity-50"
            >
              {loading ? 'Resetting...' : 'Reset password'}
            </button>

            <div className="mt-4 text-center">
              <Link to="/login" className="text-xs text-gray-600 hover:text-gray-400 transition-colors">
                Back to login
              </Link>
            </div>
          </form>
        )}
      </div>
    </div>
  );
}
