import { useState, useEffect } from 'react';
import { useParams, useNavigate } from 'react-router-dom';
import { api } from '../api/client';
import { useAuthStore } from '../stores/authStore';
import { useProjectStore } from '../stores/projectStore';
import type { ResolvedInvite } from '../api/types';

type PageState = 'loading' | 'resolved' | 'error' | 'success';

export function AcceptInvitePage() {
  const { token } = useParams<{ token: string }>();
  const navigate = useNavigate();
  const login = useAuthStore(s => s.login);

  const [state, setState] = useState<PageState>('loading');
  const [invite, setInvite] = useState<ResolvedInvite | null>(null);
  const [password, setPassword] = useState('');
  const [confirmPassword, setConfirmPassword] = useState('');
  const [error, setError] = useState('');
  const [submitting, setSubmitting] = useState(false);

  useEffect(() => {
    if (!token) {
      setState('error');
      return;
    }
    api.resolveInvite(token)
      .then(data => {
        setInvite(data);
        setState('resolved');
      })
      .catch(() => {
        setState('error');
      });
  }, [token]);

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!token || !invite) return;
    setError('');

    if (!invite.has_account) {
      if (password.length < 8) {
        setError('Password must be at least 8 characters');
        return;
      }
      if (password !== confirmPassword) {
        setError('Passwords do not match');
        return;
      }
    } else {
      if (!password) {
        setError('Password is required to verify your identity');
        return;
      }
    }

    setSubmitting(true);
    try {
      const res = invite.has_account
        ? await api.acceptInvite(token, undefined, password)
        : await api.acceptInvite(token, password);

      // Store auth and navigate to project
      login(res.token, res.email, res.role, false, 'active', false);

      // Fetch projects so project store is populated
      try {
        const projects = await api.getProjects();
        useProjectStore.getState().setProjects(projects);
        const target = projects.find(p => p.project_id === res.project_id);
        if (target) {
          useProjectStore.getState().setActiveProject(target);
        }
      } catch {
        // non-fatal
      }

      navigate(`/projects/${res.project_id}`);
    } catch (err) {
      const msg = err instanceof Error ? err.message : 'Failed to accept invite';
      setError(msg);
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <div className="min-h-screen bg-[var(--bg-base)] flex flex-col items-center pt-[15vh] p-4">
      <div className="w-full max-w-xs">
        {/* Brand */}
        <div className="text-center mb-8">
          <h1 className="text-[#4ade80] text-2xl font-bold tracking-tight mb-1">
            AletheDash
          </h1>
          <p className="text-gray-600 text-xs uppercase tracking-widest">
            network diagnostics
          </p>
        </div>

        {/* Loading */}
        {state === 'loading' && (
          <p className="text-gray-500 text-sm text-center motion-safe:animate-pulse">
            Loading invitation...
          </p>
        )}

        {/* Error */}
        {state === 'error' && (
          <div className="text-center">
            <p className="text-gray-400 text-sm mb-4">
              This invitation has expired or is no longer valid.
            </p>
            <a href="/login" className="text-xs text-cyan-400 hover:text-cyan-300 transition-colors">
              Go to login
            </a>
          </div>
        )}

        {/* Resolved */}
        {state === 'resolved' && invite && (
          <div>
            <div className="text-center mb-6">
              <p className="text-gray-500 text-sm mb-1">You've been invited to join</p>
              <p className="text-gray-100 text-lg font-medium">{invite.project_name}</p>
              <p className="text-gray-500 text-xs mt-1">
                as <span className="text-cyan-400">{invite.role}</span>
              </p>
            </div>

            <form onSubmit={handleSubmit}>
              <h3 className="text-xs text-gray-600 uppercase tracking-wider mb-4 text-center">
                {invite.has_account ? 'Sign in to accept' : 'Create your account'}
              </h3>

              {error && (
                <div className="text-red-400 text-xs mb-4 flex items-center gap-2">
                  <span className="text-red-500">err</span>
                  <span>{error}</span>
                </div>
              )}

              {/* Email (read-only) */}
              <div className="mb-4">
                <label className="block text-xs text-gray-600 mb-1.5 uppercase tracking-wider">
                  Email
                </label>
                <div className="flex items-center border-b border-gray-700">
                  <span className="text-green-600/60 text-sm mr-2 select-none">&gt;</span>
                  <input
                    type="email"
                    value={invite.email}
                    readOnly
                    className="w-full bg-transparent py-2 text-sm text-gray-400 focus:outline-none cursor-not-allowed"
                  />
                </div>
              </div>

              {/* Password */}
              <div className="mb-4">
                <label className="block text-xs text-gray-600 mb-1.5 uppercase tracking-wider">
                  Password
                </label>
                <div className="flex items-center border-b border-gray-700 focus-within:border-green-500/50 transition-colors">
                  <span className="text-green-600/60 text-sm mr-2 select-none">&gt;</span>
                  <input
                    type="password"
                    value={password}
                    onChange={e => setPassword(e.target.value)}
                    className="w-full bg-transparent py-2 text-sm text-gray-200 focus:outline-none placeholder:text-gray-700"
                    placeholder="••••••••"
                    autoFocus
                  />
                </div>
              </div>

              {/* Confirm password (new accounts only) */}
              {!invite.has_account && (
                <div className="mb-4">
                  <label className="block text-xs text-gray-600 mb-1.5 uppercase tracking-wider">
                    Confirm Password
                  </label>
                  <div className="flex items-center border-b border-gray-700 focus-within:border-green-500/50 transition-colors">
                    <span className="text-green-600/60 text-sm mr-2 select-none">&gt;</span>
                    <input
                      type="password"
                      value={confirmPassword}
                      onChange={e => setConfirmPassword(e.target.value)}
                      className="w-full bg-transparent py-2 text-sm text-gray-200 focus:outline-none placeholder:text-gray-700"
                      placeholder="••••••••"
                    />
                  </div>
                </div>
              )}

              <button
                type="submit"
                disabled={submitting}
                className="w-full bg-cyan-600 hover:bg-cyan-500 text-white py-2.5 rounded text-sm font-medium transition-colors disabled:opacity-50 mt-2"
              >
                {submitting
                  ? 'Accepting...'
                  : invite.has_account
                    ? 'Accept Invitation'
                    : 'Create Account & Join'}
              </button>
            </form>
          </div>
        )}
      </div>
    </div>
  );
}
