import { useState } from 'react';
import { Link } from 'react-router-dom';
import { api } from '../api/client';

export function ForgotPasswordPage() {
  const [email, setEmail] = useState('');
  const [sent, setSent] = useState(false);
  const [loading, setLoading] = useState(false);

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!email.trim()) return;
    setLoading(true);
    try {
      await api.forgotPassword(email.trim());
      setSent(true);
    } finally {
      setLoading(false);
    }
  };

  return (
    <div className="min-h-screen bg-[var(--bg-base)] flex items-center justify-center p-4">
      <div className="w-full max-w-xs">
        <div className="text-center mb-8">
          <h1 className="text-[#4ade80] text-2xl font-bold tracking-tight mb-1">
            AletheDash
          </h1>
          <p className="text-gray-600 text-xs uppercase tracking-widest">
            network diagnostics
          </p>
        </div>

        {sent ? (
          <div>
            <div className="text-green-400 text-xs mb-4 flex items-center gap-2">
              <span className="text-green-500">ok</span>
              <span>If that email is registered, a reset link has been sent.</span>
            </div>
            <p className="text-gray-500 text-xs mb-6">
              Check your inbox and spam folder. The link expires in 1 hour.
              If SMTP is not configured, the reset link is in the server logs.
            </p>
            <Link to="/login" className="text-xs text-cyan-400 hover:text-cyan-300">
              Back to login
            </Link>
          </div>
        ) : (
          <form onSubmit={handleSubmit}>
            <p className="text-gray-400 text-xs mb-6">
              Enter the email address associated with your account. We'll send a link to reset your password.
            </p>

            <div className="mb-6">
              <label htmlFor="reset-email" className="block text-xs text-gray-600 mb-1.5 uppercase tracking-wider">
                Email
              </label>
              <div className="flex items-center border-b border-gray-700 focus-within:border-green-500/50 transition-colors">
                <span className="text-green-600/60 text-sm mr-2 select-none">&gt;</span>
                <input
                  id="reset-email"
                  type="email"
                  value={email}
                  onChange={(e) => setEmail(e.target.value)}
                  className="w-full bg-transparent py-2 text-sm text-gray-200 focus:outline-none placeholder:text-gray-700"
                  placeholder="you@example.com"
                  autoFocus
                />
              </div>
            </div>

            <button
              type="submit"
              disabled={loading || !email.trim()}
              className="w-full bg-green-600 hover:bg-green-500 text-white py-2 rounded text-sm font-medium transition-colors disabled:opacity-50"
            >
              {loading ? 'Sending...' : 'Send reset link'}
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
