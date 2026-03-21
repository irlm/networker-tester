import { useState, useEffect } from 'react';
import { Link, useNavigate, useSearchParams } from 'react-router-dom';
import { api } from '../api/client';
import { useAuthStore } from '../stores/authStore';

const SSO_ERROR_MESSAGES: Record<string, string> = {
  sso_denied: 'SSO login was cancelled or denied.',
  missing_code: 'SSO callback missing authorization code.',
  missing_state: 'SSO callback missing state parameter.',
  state_mismatch: 'SSO state mismatch (possible CSRF). Try again.',
  token_exchange_failed: 'Failed to exchange SSO token. Try again.',
  token_parse_failed: 'Failed to parse SSO response.',
  no_id_token: 'SSO provider did not return an identity token.',
  id_token_decode: 'Failed to decode SSO identity token.',
  missing_claims: 'SSO identity token missing email or subject.',
  link_failed: 'Failed to link SSO account.',
  create_failed: 'Failed to create SSO account.',
  internal: 'An internal error occurred. Try again.',
};

export function LoginPage() {
  const [email, setEmail] = useState('');
  const [password, setPassword] = useState('');
  const [showPassword, setShowPassword] = useState(false);
  const [error, setError] = useState('');
  const [loading, setLoading] = useState(false);
  const [checkingEmail, setCheckingEmail] = useState(false);
  const [providers, setProviders] = useState<{ microsoft: boolean; google: boolean }>({
    microsoft: false,
    google: false,
  });
  const login = useAuthStore((s) => s.login);
  const navigate = useNavigate();
  const [searchParams] = useSearchParams();

  // Load SSO providers on mount
  useEffect(() => {
    api.getProviders().then(setProviders).catch(() => {
      // SSO not available, that's fine
    });
  }, []);

  // Check for SSO error in URL params
  useEffect(() => {
    const ssoError = searchParams.get('error');
    if (ssoError) {
      setError(SSO_ERROR_MESSAGES[ssoError] || `SSO error: ${ssoError}`);
    }
  }, [searchParams]);

  const hasSSO = providers.microsoft || providers.google;

  const handleEmailContinue = async () => {
    if (!email.trim()) return;

    if (!hasSSO) {
      setShowPassword(true);
      return;
    }

    setCheckingEmail(true);
    setError('');
    try {
      const result = await api.checkEmail(email);
      if (result.provider) {
        // Redirect to SSO
        window.location.href = `/api/auth/sso/${result.provider}`;
      } else {
        setShowPassword(true);
      }
    } catch {
      // Fallback to password
      setShowPassword(true);
    } finally {
      setCheckingEmail(false);
    }
  };

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    setError('');

    if (!showPassword) {
      await handleEmailContinue();
      return;
    }

    setLoading(true);
    try {
      const res = await api.login(email, password);
      login(res.token, res.email, res.role, res.must_change_password, res.status);
      if (res.must_change_password) {
        navigate('/change-password');
      } else {
        navigate('/');
      }
    } catch {
      setError('Wrong email or password');
    } finally {
      setLoading(false);
    }
  };

  const handleSSOClick = (provider: string) => {
    window.location.href = `/api/auth/sso/${provider}`;
  };

  return (
    <div className="min-h-screen bg-[var(--bg-base)] flex items-center justify-center">
      <div className="w-80">
        {/* Brand */}
        <div className="text-center mb-8">
          <h1 className="text-[#4ade80] text-2xl font-bold tracking-tight mb-1">
            Networker
          </h1>
          <p className="text-gray-600 text-xs uppercase tracking-widest">
            diagnostics platform
          </p>
        </div>

        {/* Card */}
        <div className="border border-[var(--border-default)] rounded-lg p-6 bg-[var(--bg-surface)]">
          {error && (
            <div className="text-red-400 text-xs mb-4 flex items-center gap-2">
              <span className="text-red-500">err</span>
              <span>{error}</span>
            </div>
          )}

          {/* SSO buttons */}
          {hasSSO && !showPassword && (
            <>
              {providers.microsoft && (
                <button
                  type="button"
                  onClick={() => handleSSOClick('microsoft')}
                  className="w-full flex items-center justify-center gap-2 border border-[var(--border-default)] rounded py-2.5 px-4 text-sm text-gray-200 hover:bg-white/5 transition-colors mb-3"
                >
                  <svg width="16" height="16" viewBox="0 0 21 21" fill="none">
                    <rect x="1" y="1" width="9" height="9" fill="#F25022" />
                    <rect x="11" y="1" width="9" height="9" fill="#7FBA00" />
                    <rect x="1" y="11" width="9" height="9" fill="#00A4EF" />
                    <rect x="11" y="11" width="9" height="9" fill="#FFB900" />
                  </svg>
                  Continue with Microsoft
                </button>
              )}
              {providers.google && (
                <button
                  type="button"
                  onClick={() => handleSSOClick('google')}
                  className="w-full flex items-center justify-center gap-2 border border-[var(--border-default)] rounded py-2.5 px-4 text-sm text-gray-200 hover:bg-white/5 transition-colors mb-3"
                >
                  <svg width="16" height="16" viewBox="0 0 48 48">
                    <path fill="#EA4335" d="M24 9.5c3.54 0 6.71 1.22 9.21 3.6l6.85-6.85C35.9 2.38 30.47 0 24 0 14.62 0 6.51 5.38 2.56 13.22l7.98 6.19C12.43 13.72 17.74 9.5 24 9.5z" />
                    <path fill="#4285F4" d="M46.98 24.55c0-1.57-.15-3.09-.38-4.55H24v9.02h12.94c-.58 2.96-2.26 5.48-4.78 7.18l7.73 6c4.51-4.18 7.09-10.36 7.09-17.65z" />
                    <path fill="#FBBC05" d="M10.53 28.59c-.48-1.45-.76-2.99-.76-4.59s.27-3.14.76-4.59l-7.98-6.19C.92 16.46 0 20.12 0 24c0 3.88.92 7.54 2.56 10.78l7.97-6.19z" />
                    <path fill="#34A853" d="M24 48c6.48 0 11.93-2.13 15.89-5.81l-7.73-6c-2.15 1.45-4.92 2.3-8.16 2.3-6.26 0-11.57-4.22-13.47-9.91l-7.98 6.19C6.51 42.62 14.62 48 24 48z" />
                  </svg>
                  Continue with Google
                </button>
              )}

              {/* Divider */}
              <div className="flex items-center gap-3 my-4">
                <div className="flex-1 h-px bg-[var(--border-default)]" />
                <span className="text-xs text-gray-600 uppercase tracking-wider">or</span>
                <div className="flex-1 h-px bg-[var(--border-default)]" />
              </div>
            </>
          )}

          {/* Form */}
          <form onSubmit={handleSubmit}>
            <div className="mb-4">
              <label htmlFor="login-email" className="block text-xs text-gray-600 mb-1.5 uppercase tracking-wider">
                Email
              </label>
              <div className="flex items-center border-b border-gray-700 focus-within:border-green-500/50 transition-colors">
                <span className="text-green-600/60 text-sm mr-2 select-none">&gt;</span>
                <input
                  id="login-email"
                  type="email"
                  value={email}
                  onChange={(e) => {
                    setEmail(e.target.value);
                    if (showPassword) setShowPassword(false);
                  }}
                  className="w-full bg-transparent py-2 text-sm text-gray-200 focus:outline-none placeholder:text-gray-700"
                  placeholder="you@company.com"
                  autoFocus
                />
              </div>
            </div>

            {showPassword && (
              <div className="mb-4">
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
                    placeholder="password"
                    autoFocus
                  />
                </div>
              </div>
            )}

            <button
              type="submit"
              disabled={loading || checkingEmail}
              className="w-full bg-green-600 hover:bg-green-500 text-white py-2.5 rounded text-sm font-medium transition-colors disabled:opacity-50 mt-4"
            >
              {loading
                ? 'Signing in...'
                : checkingEmail
                  ? 'Checking...'
                  : showPassword
                    ? 'Sign in'
                    : 'Continue'}
            </button>

            {showPassword && (
              <div className="mt-4 text-center">
                <Link to="/forgot-password" className="text-xs text-gray-600 hover:text-gray-400 transition-colors">
                  Forgot password?
                </Link>
              </div>
            )}
          </form>
        </div>
      </div>
    </div>
  );
}
